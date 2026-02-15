//! ListDirectoryTool — list directory contents in the repository.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.

use std::path::{Path, PathBuf};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

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
#[derive(Debug, Serialize)]
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
            description: "List the contents of a directory in the repository. \
                Returns file and subdirectory names with sizes. \
                Useful for understanding project structure."
                .to_string(),
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
        let entries = list_directory(&self.repo_root, &args.path)
            .await
            .map_err(ListDirectoryError)?;

        // Format as a readable listing for the LLM
        if entries.is_empty() {
            return Ok("Directory is empty.".to_string());
        }

        let formatted: Vec<String> = entries
            .iter()
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.name)
                } else if let Some(size) = e.size {
                    format!("{} ({} bytes)", e.name, size)
                } else {
                    e.name.clone()
                }
            })
            .collect();

        Ok(formatted.join("\n"))
    }
}

/// List the contents of a directory in the repository.
///
/// Returns entries sorted with directories first, then files.
pub async fn list_directory(
    repo_root: &Path,
    relative_path: &str,
) -> Result<Vec<DirEntry>, String> {
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

    let mut entries = Vec::new();
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
        let is_dir = metadata.as_ref().map_or(false, |m| m.is_dir());
        let size = if is_dir {
            None
        } else {
            metadata.map(|m| m.len())
        };

        entries.push(DirEntry { name, is_dir, size });
    }

    // Sort: directories first, then by name
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));

    Ok(entries)
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

        let entries = list_directory(dir.path(), ".").await.unwrap();

        // Should have subdir and file.txt, but not .hidden
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir); // directories first
        assert_eq!(entries[0].name, "subdir");
        assert_eq!(entries[1].name, "file.txt");
    }

    #[tokio::test]
    async fn list_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let entries = list_directory(dir.path(), ".").await.unwrap();
        assert!(entries.is_empty());
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
        assert!(result.unwrap_err().contains("traversal"), "should block path traversal");
    }

    #[tokio::test]
    async fn call_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = Tool::call(&tool, ListDirectoryArgs { path: ".".to_string() }).await.unwrap();
        assert_eq!(result, "Directory is empty.");
    }

    #[tokio::test]
    async fn call_with_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let tool = ListDirectoryTool::new(dir.path().to_path_buf());
        let result = Tool::call(&tool, ListDirectoryArgs { path: ".".to_string() }).await.unwrap();
        assert!(result.contains("src/"));
        assert!(result.contains("main.rs"));
        assert!(result.contains("bytes"));
    }

    #[tokio::test]
    async fn definition_has_correct_name() {
        let tool = ListDirectoryTool::new(PathBuf::from("/tmp"));
        let def = Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "list_directory");
        assert!(!def.description.is_empty());
    }
}
