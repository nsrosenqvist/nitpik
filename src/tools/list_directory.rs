//! ListDirectoryTool — list directory contents in the repository.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.

use std::path::{Path, PathBuf};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::format::format_byte_size;

/// Maximum number of entries returned in a single listing.
///
/// Large directories (e.g. `node_modules`) would otherwise dump
/// thousands of names into the LLM's context. The cap keeps responses
/// predictable; the truncation footer tells the model exactly how
/// many were dropped.
const MAX_ENTRIES: usize = 200;

/// Arguments for the list_directory tool.
#[derive(Debug, Deserialize)]
pub struct ListDirectoryArgs {
    /// Relative path to the directory within the repository.
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_path() -> String {
    ".".to_string()
}

/// Error type for the list_directory tool.
#[derive(Debug, thiserror::Error)]
#[error("ListDirectory error: {0}")]
pub struct ListDirectoryError(pub String);

/// A directory entry.
#[derive(Debug, Clone, Serialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Rig-core tool that lists directory contents in the repository.
#[derive(Serialize, Deserialize)]
pub struct ListDirectoryTool {
    repo_root: PathBuf,
}

impl ListDirectoryTool {
    /// Create a new ListDirectoryTool anchored at the given repo root.
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

impl Tool for ListDirectoryTool {
    const NAME: &'static str = "list_directory";
    type Error = ListDirectoryError;
    type Args = ListDirectoryArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "list_directory".to_string(),
            description: format!(
                "List the contents of a directory in the repository. \
                 Returns subdirectories first (suffixed with `/`), then \
                 files with sizes. Hidden entries (names starting with \
                 `.`) are skipped. Up to {MAX_ENTRIES} entries are \
                 returned per call; if the directory has more, the \
                 listing is truncated and a footer reports the total. \
                 Useful for understanding project structure."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the directory within the repository. Use '.' or omit to list the repo root.",
                        "default": "."
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Err(msg) = crate::tools::budget::try_consume("list_directory") {
            return Err(ListDirectoryError(msg));
        }
        let start = crate::tools::start_tool_call();
        let listing = list_directory(&self.repo_root, &args.path)
            .await
            .map_err(ListDirectoryError)?;

        let truncated_marker = if listing.truncated {
            format!(" (+{} more)", listing.total_entries - listing.entries.len())
        } else {
            String::new()
        };
        let result_summary = format!(
            "{} entr{}{truncated_marker}",
            listing.entries.len(),
            if listing.entries.len() == 1 {
                "y"
            } else {
                "ies"
            }
        );
        crate::tools::finish_tool_call(start, "list_directory", &args.path, result_summary);

        if listing.entries.is_empty() && !listing.truncated {
            return Ok("Directory is empty.".to_string());
        }

        let mut formatted: Vec<String> = listing
            .entries
            .iter()
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.name)
                } else if let Some(size) = e.size {
                    format!("{} ({})", e.name, format_byte_size(size as usize))
                } else {
                    e.name.clone()
                }
            })
            .collect();

        if listing.truncated {
            formatted.push(format!(
                "... showing {} of {} entries; narrow the path to see more",
                listing.entries.len(),
                listing.total_entries,
            ));
        }

        Ok(formatted.join("\n"))
    }
}

/// Outcome of a [`list_directory`] call.
#[derive(Debug, Clone)]
pub struct DirListing {
    /// Entries returned (capped at [`MAX_ENTRIES`]), directories first.
    pub entries: Vec<DirEntry>,
    /// Total number of (non-hidden) entries in the directory.
    pub total_entries: usize,
    /// True when [`MAX_ENTRIES`] was hit.
    pub truncated: bool,
}

/// List the contents of a directory in the repository.
///
/// Returns entries sorted with directories first, then files. Capped
/// at [`MAX_ENTRIES`]; the [`DirListing`] reports the true total so
/// callers can render an honest truncation footer.
pub async fn list_directory(repo_root: &Path, relative_path: &str) -> Result<DirListing, String> {
    let full_path = repo_root.join(relative_path);

    // Security: ensure path is within repo
    let canonical = full_path
        .canonicalize()
        .map_err(|e| format!("directory not found: {relative_path} ({e})"))?;

    let repo_canonical = repo_root
        .canonicalize()
        .map_err(|e| format!("invalid repo root: {e}"))?;

    if !canonical.starts_with(&repo_canonical) {
        return Err(format!("path traversal blocked: {relative_path}"));
    }

    let mut entries: Vec<DirEntry> = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&canonical)
        .await
        .map_err(|e| format!("cannot read directory: {e}"))?;

    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| format!("error reading entry: {e}"))?
    {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/directories
        if name.starts_with('.') {
            continue;
        }

        let metadata = entry.metadata().await.ok();
        let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());
        let size = if is_dir {
            None
        } else {
            metadata.map(|m| m.len())
        };

        entries.push(DirEntry { name, is_dir, size });
    }

    // Sort: directories first, then by name. Done before truncation so
    // the cap consistently keeps the alphabetically earliest entries
    // (and prefers directories) regardless of filesystem iteration order.
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));

    let total_entries = entries.len();
    let truncated = total_entries > MAX_ENTRIES;
    if truncated {
        entries.truncate(MAX_ENTRIES);
    }

    Ok(DirListing {
        entries,
        total_entries,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_directory_contents() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "content").unwrap();
        std::fs::write(dir.path().join(".hidden"), "hidden").unwrap();

        let listing = list_directory(dir.path(), ".").await.unwrap();

        // Should have subdir and file.txt, but not .hidden
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.total_entries, 2);
        assert!(!listing.truncated);
        assert!(listing.entries[0].is_dir); // directories first
        assert_eq!(listing.entries[0].name, "subdir");
        assert_eq!(listing.entries[1].name, "file.txt");
    }

    #[tokio::test]
    async fn list_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let listing = list_directory(dir.path(), ".").await.unwrap();
        assert!(listing.entries.is_empty());
        assert_eq!(listing.total_entries, 0);
        assert!(!listing.truncated);
    }

    #[tokio::test]
    async fn list_nonexistent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = list_directory(dir.path(), "no_such_dir").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_directory_path_traversal_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let result = list_directory(dir.path(), "../../../etc").await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("traversal"),
            "should block path traversal"
        );
    }

    #[tokio::test]
    async fn list_directory_truncates_at_cap() {
        let dir = tempfile::tempdir().unwrap();
        // Create MAX_ENTRIES + 5 files so we know the cap is exercised
        // without making the test painfully slow on CI.
        let count = MAX_ENTRIES + 5;
        for i in 0..count {
            std::fs::write(dir.path().join(format!("f{i:04}.txt")), "x").unwrap();
        }

        let listing = list_directory(dir.path(), ".").await.unwrap();
        assert_eq!(listing.entries.len(), MAX_ENTRIES);
        assert_eq!(listing.total_entries, count);
        assert!(listing.truncated);
        // Sorted alphabetically, first entry is f0000.txt.
        assert_eq!(listing.entries[0].name, "f0000.txt");
    }

    #[tokio::test]
    async fn call_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            ListDirectoryArgs {
                path: ".".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "Directory is empty.");
    }

    #[tokio::test]
    async fn call_with_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            ListDirectoryArgs {
                path: ".".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(result.contains("src/"));
        assert!(result.contains("main.rs"));
        // format_byte_size renders 12 bytes as "12B".
        assert!(result.contains("12B"));
    }

    #[tokio::test]
    async fn call_renders_truncation_footer() {
        let dir = tempfile::tempdir().unwrap();
        let count = MAX_ENTRIES + 3;
        for i in 0..count {
            std::fs::write(dir.path().join(format!("f{i:04}.txt")), "x").unwrap();
        }

        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            ListDirectoryArgs {
                path: ".".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(result.contains(&format!("showing {MAX_ENTRIES} of {count} entries")));
    }

    #[tokio::test]
    async fn definition_has_correct_name() {
        let tool = ListDirectoryTool::new(PathBuf::from("/tmp"));
        let def = Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "list_directory");
        assert!(!def.description.is_empty());
        assert!(def.description.contains("truncated"));
    }
}
