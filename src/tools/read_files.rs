//! ReadFilesTool — batched read of multiple files in a single call.
//!
//! Implements rig-core's `Tool` trait. Reuses
//! [`crate::tools::read_file::read_file`] under the hood so path
//! sandboxing, line numbering, and truncation behavior are identical
//! to the single-file `read_file` tool.

use std::path::PathBuf;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::format::format_byte_size;
use crate::tools::read_file::read_file;

/// Maximum number of files accepted in a single batch.
const MAX_BATCH: usize = 10;

/// Soft cap on total bytes returned in one batch response. When the
/// budget is exhausted, remaining files are skipped and a footer
/// reports how many were dropped.
const BATCH_BYTE_BUDGET: usize = 64 * 1024;

/// One file entry in a batch read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFilesEntry {
    /// Relative path within the repository.
    pub path: String,
    /// Optional 1-based start line (inclusive).
    #[serde(default)]
    pub start_line: Option<usize>,
    /// Optional 1-based end line (inclusive).
    #[serde(default)]
    pub end_line: Option<usize>,
}

/// Arguments for the read_files tool.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReadFilesArgs {
    /// List of files to read in this batch.
    pub files: Vec<ReadFilesEntry>,
}

/// Error type for the read_files tool.
#[derive(Debug, thiserror::Error)]
#[error("ReadFiles error: {0}")]
pub struct ReadFilesError(pub String);

/// Rig-core tool that reads multiple files in one call.
#[derive(Serialize, Deserialize)]
pub struct ReadFilesTool {
    repo_root: PathBuf,
}

impl ReadFilesTool {
    /// Create a new ReadFilesTool anchored at the given repo root.
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

impl Tool for ReadFilesTool {
    const NAME: &'static str = "read_files";
    type Error = ReadFilesError;
    type Args = ReadFilesArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "read_files".to_string(),
            description: format!(
                "Read multiple files in a single call. Accepts up to \
                 {MAX_BATCH} files; each entry can specify optional \
                 `start_line`/`end_line` to read a slice. Output for \
                 each file is preceded by a `### path` header and \
                 line-numbered identically to `read_file`.\n\n\
                 The total response is capped at {} of content; once \
                 the budget is hit, remaining files are skipped and a \
                 footer reports how many were dropped. Use this when \
                 you need to gather context from several known paths \
                 — it saves the per-call overhead of repeated \
                 `read_file` invocations.",
                format_byte_size(BATCH_BYTE_BUDGET),
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_BATCH,
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": {
                                    "type": "string",
                                    "description": "Relative path within the repo (e.g. 'src/main.rs')."
                                },
                                "start_line": {
                                    "type": "integer",
                                    "description": "Optional 1-based starting line (inclusive)."
                                },
                                "end_line": {
                                    "type": "integer",
                                    "description": "Optional 1-based ending line (inclusive)."
                                }
                            },
                            "required": ["path"]
                        }
                    }
                },
                "required": ["files"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Err(msg) = crate::tools::budget::try_consume("read_files") {
            return Err(ReadFilesError(msg));
        }
        let start = crate::tools::start_tool_call();

        if args.files.is_empty() {
            crate::tools::finish_tool_call(start, "read_files", "", "no files");
            return Err(ReadFilesError("no files supplied".into()));
        }
        let total_requested = args.files.len();
        let files: Vec<ReadFilesEntry> = args.files.into_iter().take(MAX_BATCH).collect();

        let mut sections: Vec<String> = Vec::with_capacity(files.len());
        let mut bytes_used: usize = 0;
        let mut dropped = 0usize;
        let mut errors = 0usize;

        for entry in &files {
            if bytes_used >= BATCH_BYTE_BUDGET {
                dropped += 1;
                continue;
            }
            let header = format!("### {}\n", entry.path);
            match read_file(
                &self.repo_root,
                &entry.path,
                entry.start_line,
                entry.end_line,
            )
            .await
            {
                Ok(out) => {
                    sections.push(format!("{header}{}", out.content));
                    bytes_used += header.len() + out.content.len();
                }
                Err(e) => {
                    errors += 1;
                    sections.push(format!("{header}error: {e}"));
                }
            }
        }

        if dropped > 0 || total_requested > MAX_BATCH {
            let extra = total_requested.saturating_sub(MAX_BATCH);
            sections.push(format!(
                "\n... {dropped} file(s) skipped due to byte budget; \
                 {extra} file(s) over the per-call limit of {MAX_BATCH}"
            ));
        }

        let summary = format!(
            "{}/{total_requested} read{}",
            files.len() - dropped,
            if errors > 0 {
                format!(", {errors} error(s)")
            } else {
                String::new()
            }
        );
        let arg_summary = files
            .iter()
            .take(3)
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        crate::tools::finish_tool_call(start, "read_files", &arg_summary, summary);

        Ok(sections.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::write(
            p.join("src/a.rs"),
            "line 1 of a\nline 2 of a\nline 3 of a\n",
        )
        .unwrap();
        fs::write(p.join("src/b.rs"), "line 1 of b\nline 2 of b\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn read_files_returns_each_section_with_header() {
        let dir = make_repo();
        let tool = ReadFilesTool::new(dir.path().to_path_buf());
        let out = tool
            .call(ReadFilesArgs {
                files: vec![
                    ReadFilesEntry {
                        path: "src/a.rs".into(),
                        start_line: None,
                        end_line: None,
                    },
                    ReadFilesEntry {
                        path: "src/b.rs".into(),
                        start_line: None,
                        end_line: None,
                    },
                ],
            })
            .await
            .unwrap();
        assert!(out.contains("### src/a.rs"));
        assert!(out.contains("### src/b.rs"));
        assert!(out.contains("line 1 of a"));
        assert!(out.contains("line 1 of b"));
    }

    #[tokio::test]
    async fn read_files_reports_per_file_errors() {
        let dir = make_repo();
        let tool = ReadFilesTool::new(dir.path().to_path_buf());
        let out = tool
            .call(ReadFilesArgs {
                files: vec![ReadFilesEntry {
                    path: "src/missing.rs".into(),
                    start_line: None,
                    end_line: None,
                }],
            })
            .await
            .unwrap();
        assert!(out.contains("### src/missing.rs"));
        assert!(out.contains("error:"));
    }

    #[tokio::test]
    async fn read_files_supports_line_ranges() {
        let dir = make_repo();
        let tool = ReadFilesTool::new(dir.path().to_path_buf());
        let out = tool
            .call(ReadFilesArgs {
                files: vec![ReadFilesEntry {
                    path: "src/a.rs".into(),
                    start_line: Some(2),
                    end_line: Some(2),
                }],
            })
            .await
            .unwrap();
        assert!(out.contains("line 2 of a"));
        assert!(!out.contains("line 1 of a"));
        assert!(!out.contains("line 3 of a"));
    }

    #[tokio::test]
    async fn read_files_rejects_empty_input() {
        let dir = make_repo();
        let tool = ReadFilesTool::new(dir.path().to_path_buf());
        let err = tool
            .call(ReadFilesArgs { files: vec![] })
            .await
            .unwrap_err();
        assert!(err.0.contains("no files"));
    }
}
