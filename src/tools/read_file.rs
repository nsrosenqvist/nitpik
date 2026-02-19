//! ReadFileTool — reads a file from the repository.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.

use std::path::{Path, PathBuf};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Maximum file size to read (1MB).
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Arguments for the read_file tool.
#[derive(Debug, Deserialize)]
pub struct ReadFileArgs {
    /// Relative path to the file within the repository.
    pub path: String,
    /// Optional 1-based start line (inclusive). Omit to start from the beginning.
    pub start_line: Option<usize>,
    /// Optional 1-based end line (inclusive). Omit to read to the end.
    pub end_line: Option<usize>,
}

/// Error type for the read_file tool.
#[derive(Debug, thiserror::Error)]
#[error("ReadFile error: {0}")]
pub struct ReadFileError(pub String);

/// Rig-core tool that reads a file from the repository.
///
/// Holds a reference to the repo root directory. Path traversal
/// outside the repo is blocked.
#[derive(Serialize, Deserialize)]
pub struct ReadFileTool {
    repo_root: PathBuf,
}

impl ReadFileTool {
    /// Create a new ReadFileTool anchored at the given repo root.
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

impl Tool for ReadFileTool {
    const NAME: &'static str = "read_file";
    type Error = ReadFileError;
    type Args = ReadFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file from the repository. \
                Use this to examine source code, configuration files, or documentation. \
                You can optionally specify a line range to read only a portion of the file, \
                which is more efficient for large files. When no range is given the full \
                file is returned."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the repository (e.g., 'src/main.rs')"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "1-based starting line number (inclusive). Omit to start from the beginning of the file."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "1-based ending line number (inclusive). Omit to read to the end of the file."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let start = crate::tools::start_tool_call();
        let result = read_file(&self.repo_root, &args.path, args.start_line, args.end_line).await;
        let range_suffix = match (args.start_line, args.end_line) {
            (Some(s), Some(e)) => format!(" L{s}-{e}"),
            (Some(s), None) => format!(" L{s}-end"),
            (None, Some(e)) => format!(" L1-{e}"),
            (None, None) => String::new(),
        };
        let summary = match &result {
            Ok(content) => format!("{}{range_suffix}", format_byte_size(content.len())),
            Err(e) => format!("error: {e}"),
        };
        crate::tools::finish_tool_call(start, "read_file", &args.path, summary);
        result.map_err(ReadFileError)
    }
}

/// Read a file from the repository, with path sanitization and size limits.
///
/// When `start_line` and/or `end_line` are provided the returned content is
/// limited to the requested 1-based inclusive line range. Lines outside the
/// file are silently clamped.
pub async fn read_file(
    repo_root: &Path,
    relative_path: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, String> {
    let sanitized = sanitize_path(relative_path);
    let full_path = repo_root.join(&sanitized);

    // Security: ensure the resolved path is within repo_root
    let canonical = full_path
        .canonicalize()
        .map_err(|e| format!("file not found: {} ({e})", sanitized.display()))?;

    let repo_canonical = repo_root
        .canonicalize()
        .map_err(|e| format!("invalid repo root: {e}"))?;

    if !canonical.starts_with(&repo_canonical) {
        return Err(format!("path traversal blocked: {}", sanitized.display()));
    }

    // Check file size
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| format!("cannot read file metadata: {e}"))?;

    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!(
            "file too large: {} bytes (max {MAX_FILE_SIZE})",
            metadata.len()
        ));
    }

    let content = tokio::fs::read_to_string(&canonical)
        .await
        .map_err(|e| format!("cannot read file: {e}"))?;

    // If no line range requested, return the full content.
    if start_line.is_none() && end_line.is_none() {
        return Ok(content);
    }

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    // Clamp to valid 1-based range.
    let start = start_line.unwrap_or(1).max(1);
    let end = end_line.unwrap_or(total).min(total);

    if start > total {
        return Ok(String::new());
    }

    // Convert 1-based inclusive to 0-based slice range.
    let selected = &lines[start - 1..end];
    Ok(selected.join("\n"))
}

/// Format a byte count as a human-readable size string.
fn format_byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Sanitize a relative path to prevent directory traversal.
fn sanitize_path(path: &str) -> PathBuf {
    let path = path.replace('\\', "/");
    let mut result = PathBuf::new();

    for component in path.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                result.pop();
            }
            c => result.push(c),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_normal_path() {
        assert_eq!(sanitize_path("src/main.rs"), PathBuf::from("src/main.rs"));
    }

    #[test]
    fn sanitize_traversal() {
        assert_eq!(
            sanitize_path("../../../etc/passwd"),
            PathBuf::from("etc/passwd")
        );
    }

    #[test]
    fn sanitize_dot_segments() {
        assert_eq!(
            sanitize_path("./src/../src/main.rs"),
            PathBuf::from("src/main.rs")
        );
    }

    #[tokio::test]
    async fn read_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();

        let content = read_file(dir.path(), "test.txt", None, None).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn read_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_file(dir.path(), "nope.txt", None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_path_traversal_blocked() {
        let dir = tempfile::tempdir().unwrap();
        // Create a file outside the repo root
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret data").unwrap();

        // Try to traverse out. The sanitize_path strips .., but canonical check
        // should still block if a symlink or other trick is used.
        let result = read_file(dir.path(), "../../../etc/passwd", None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let large_file = dir.path().join("huge.bin");
        // Create a file larger than 1MB
        let data = vec![b'x'; 1024 * 1024 + 1];
        std::fs::write(&large_file, &data).unwrap();

        let result = read_file(dir.path(), "huge.bin", None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too large"));
    }

    #[tokio::test]
    async fn call_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();

        let tool = ReadFileTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            ReadFileArgs {
                path: "test.rs".to_string(),
                start_line: None,
                end_line: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "fn main() {}");
    }

    #[tokio::test]
    async fn definition_has_correct_name() {
        let tool = ReadFileTool::new(PathBuf::from("/tmp"));
        let def = Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "read_file");
        assert!(!def.description.is_empty());
    }

    #[tokio::test]
    async fn read_line_range_middle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lines.txt"),
            "line1\nline2\nline3\nline4\nline5",
        )
        .unwrap();

        let content = read_file(dir.path(), "lines.txt", Some(2), Some(4))
            .await
            .unwrap();
        assert_eq!(content, "line2\nline3\nline4");
    }

    #[tokio::test]
    async fn read_line_range_start_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lines.txt"),
            "line1\nline2\nline3\nline4\nline5",
        )
        .unwrap();

        let content = read_file(dir.path(), "lines.txt", Some(3), None)
            .await
            .unwrap();
        assert_eq!(content, "line3\nline4\nline5");
    }

    #[tokio::test]
    async fn read_line_range_end_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lines.txt"),
            "line1\nline2\nline3\nline4\nline5",
        )
        .unwrap();

        let content = read_file(dir.path(), "lines.txt", None, Some(2))
            .await
            .unwrap();
        assert_eq!(content, "line1\nline2");
    }

    #[tokio::test]
    async fn read_line_range_clamped_beyond_end() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lines.txt"), "line1\nline2\nline3").unwrap();

        let content = read_file(dir.path(), "lines.txt", Some(2), Some(100))
            .await
            .unwrap();
        assert_eq!(content, "line2\nline3");
    }

    #[tokio::test]
    async fn read_line_range_start_beyond_end_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lines.txt"), "line1\nline2").unwrap();

        let content = read_file(dir.path(), "lines.txt", Some(99), None)
            .await
            .unwrap();
        assert_eq!(content, "");
    }

    #[tokio::test]
    async fn read_single_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lines.txt"), "line1\nline2\nline3").unwrap();

        let content = read_file(dir.path(), "lines.txt", Some(2), Some(2))
            .await
            .unwrap();
        assert_eq!(content, "line2");
    }
}
