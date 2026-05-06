//! ReadFileTool — reads a file from the repository.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.

use std::path::{Path, PathBuf};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::format::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, format_byte_size, format_with_line_numbers,
    truncate_lines,
};

/// Maximum file size to read from disk (10 MB).
///
/// Files larger than this are rejected outright — they're almost
/// certainly binary or generated artifacts the LLM has no business
/// reading. The post-read output is further capped via
/// [`crate::tools::format`] limits so a 5 MB minified file still
/// returns a manageable slice.
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Default number of lines returned when no range is specified.
///
/// Mirrors the soft cap used by Claude Code / Aider: large enough
/// for most real source files, small enough to leave room in the
/// context window for the diff and other context.
const DEFAULT_LINE_BUDGET: usize = DEFAULT_MAX_LINES;

/// Soft cap on bytes emitted in a single `read_file` response.
///
/// When a requested range exceeds this, the tail is dropped and a
/// truncation footer instructs the LLM to page using `start_line` /
/// `end_line`.
const OUTPUT_BYTE_BUDGET: usize = DEFAULT_MAX_BYTES;

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
            description: format!(
                "Read the contents of a file from the repository. Output is \
                 prefixed with 1-based line numbers (`NNNNN | line`) so you \
                 can reference exact locations.\n\n\
                 Each call returns up to {DEFAULT_LINE_BUDGET} lines or \
                 ~{} of content. When a file is larger, the tail is \
                 truncated and a footer tells you the total line count — \
                 page through it by passing `start_line` and `end_line`.\n\n\
                 Files larger than {} on disk are rejected; assume they are \
                 binary or generated.",
                format_byte_size(OUTPUT_BYTE_BUDGET),
                format_byte_size(MAX_FILE_SIZE as usize),
            ),
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
                        "description": "1-based ending line number (inclusive). Omit to read the default budget of lines from start_line."
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
            Ok(output) => {
                let truncated_marker = if output.truncated { " [truncated]" } else { "" };
                format!(
                    "{}{range_suffix}{truncated_marker}",
                    format_byte_size(output.content.len())
                )
            }
            Err(e) => format!("error: {e}"),
        };
        crate::tools::finish_tool_call(start, "read_file", &args.path, summary);
        result.map(|o| o.content).map_err(ReadFileError)
    }
}

/// Result of a successful [`read_file`] call.
#[derive(Debug, Clone)]
pub struct ReadFileOutput {
    /// Rendered output: line-numbered content, optionally followed by
    /// a truncation footer.
    pub content: String,
    /// `true` when the output was clipped by line or byte caps.
    pub truncated: bool,
    /// Total number of lines in the underlying file.
    pub total_lines: usize,
    /// Number of lines included in `content`.
    pub lines_included: usize,
    /// 1-based line number of the first line in `content`.
    pub start_line: usize,
}

/// Read a file from the repository, with path sanitization, size limits,
/// line-numbered output, and truncation.
///
/// When neither `start_line` nor `end_line` are provided the read starts
/// at line 1 and emits up to [`DEFAULT_LINE_BUDGET`] lines. When the
/// requested range exceeds the line or byte budgets the output is
/// clipped and a footer tells the caller how to page.
pub async fn read_file(
    repo_root: &Path,
    relative_path: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<ReadFileOutput, String> {
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

    // Check file size on disk — reject artifacts outright.
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| format!("cannot read file metadata: {e}"))?;

    if metadata.len() > MAX_FILE_SIZE {
        return Err(format!(
            "file too large: {} bytes (max {MAX_FILE_SIZE})",
            metadata.len()
        ));
    }

    let raw = tokio::fs::read_to_string(&canonical)
        .await
        .map_err(|e| format!("cannot read file: {e}"))?;

    let lines: Vec<&str> = raw.lines().collect();
    let total = lines.len();

    if total == 0 {
        return Ok(ReadFileOutput {
            content: String::new(),
            truncated: false,
            total_lines: 0,
            lines_included: 0,
            start_line: 1,
        });
    }

    // Resolve the requested 1-based inclusive range. When neither bound
    // is given we apply the default line budget starting from line 1.
    let req_start = start_line.unwrap_or(1).max(1);
    let req_end = match end_line {
        Some(e) => e.min(total),
        None => total.min(
            req_start
                .saturating_add(DEFAULT_LINE_BUDGET)
                .saturating_sub(1),
        ),
    };

    if req_start > total {
        return Ok(ReadFileOutput {
            content: format!(
                "(file has {total} lines; requested start_line={req_start} is past end)"
            ),
            truncated: false,
            total_lines: total,
            lines_included: 0,
            start_line: req_start,
        });
    }

    // 1-based inclusive → 0-based slice range.
    let slice = &lines[req_start - 1..req_end];
    let owned: Vec<String> = slice.iter().map(|s| (*s).to_string()).collect();

    // Apply soft byte cap so a 100 MB file with 80 lines doesn't blow
    // up the LLM's context. The caller can always page further.
    let total_lines_for_footer = total;
    let req_start_for_footer = req_start;
    let (joined, outcome) = truncate_lines(&owned, owned.len(), OUTPUT_BYTE_BUDGET, |o| {
        format!(
            "... showing lines {start}-{shown_end} of {total}; pass start_line/end_line to read more",
            start = req_start_for_footer,
            shown_end = req_start_for_footer + o.lines_included.saturating_sub(1),
            total = total_lines_for_footer,
        )
    });

    // Number the included lines, then append the footer (already in
    // `joined` if truncation happened — re-extract).
    let (numbered_body, footer) = if outcome.truncated {
        // truncate_lines appended the footer to `joined`; we need the
        // body alone for line-numbering, so split on the last newline
        // before the footer marker.
        let body_end = joined.rfind("\n... showing lines").unwrap_or(joined.len());
        let body = &joined[..body_end];
        let foot = &joined[body_end..];
        (body.to_string(), foot.trim_start_matches('\n').to_string())
    } else {
        (joined, String::new())
    };

    let mut rendered = format_with_line_numbers(&numbered_body, req_start);
    if outcome.truncated {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&footer);
    } else if total > req_end {
        // Not "truncated" by our budget but the user asked for less than
        // the whole file — surface that so they know there's more.
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&format!(
            "... showing lines {req_start}-{req_end} of {total}; pass start_line/end_line to read more"
        ));
    }

    let truncated = outcome.truncated || total > req_end || req_start > 1;

    Ok(ReadFileOutput {
        content: rendered,
        truncated,
        total_lines: total,
        lines_included: outcome.lines_included,
        start_line: req_start,
    })
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
    async fn read_existing_file_is_line_numbered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();

        let out = read_file(dir.path(), "test.txt", None, None).await.unwrap();
        assert_eq!(out.content, "    1 | hello world");
        assert!(out.fits_for_test());
        assert_eq!(out.total_lines, 1);
        assert_eq!(out.lines_included, 1);
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
        let result = read_file(dir.path(), "../../../etc/passwd", None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_file_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let large_file = dir.path().join("huge.bin");
        // Create a file just past the on-disk hard cap.
        let data = vec![b'x'; (MAX_FILE_SIZE + 1) as usize];
        std::fs::write(&large_file, &data).unwrap();

        let result = read_file(dir.path(), "huge.bin", None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too large"));
    }

    #[tokio::test]
    async fn call_returns_line_numbered_content() {
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
        assert_eq!(result, "    1 | fn main() {}");
    }

    #[tokio::test]
    async fn definition_has_correct_name_and_mentions_paging() {
        let tool = ReadFileTool::new(PathBuf::from("/tmp"));
        let def = Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "read_file");
        assert!(!def.description.is_empty());
        assert!(def.description.contains("start_line"));
        assert!(def.description.contains("page"));
    }

    #[tokio::test]
    async fn read_line_range_middle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lines.txt"),
            "line1\nline2\nline3\nline4\nline5",
        )
        .unwrap();

        let out = read_file(dir.path(), "lines.txt", Some(2), Some(4))
            .await
            .unwrap();
        assert!(
            out.content
                .starts_with("    2 | line2\n    3 | line3\n    4 | line4")
        );
        // Range was a strict subset → footer mentions paging.
        assert!(out.content.contains("showing lines 2-4 of 5"));
        assert!(out.truncated);
    }

    #[tokio::test]
    async fn read_line_range_start_only_uses_default_budget() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(dir.path().join("lines.txt"), body).unwrap();

        let out = read_file(dir.path(), "lines.txt", Some(3), None)
            .await
            .unwrap();
        // Default budget is large; should reach end.
        assert!(out.content.contains("    3 | line3"));
        assert!(out.content.contains("   10 | line10"));
    }

    #[tokio::test]
    async fn read_line_range_end_only_starts_at_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lines.txt"),
            "line1\nline2\nline3\nline4\nline5",
        )
        .unwrap();

        let out = read_file(dir.path(), "lines.txt", None, Some(2))
            .await
            .unwrap();
        assert!(out.content.starts_with("    1 | line1\n    2 | line2"));
        assert!(out.content.contains("showing lines 1-2 of 5"));
    }

    #[tokio::test]
    async fn read_line_range_clamped_beyond_end() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lines.txt"), "line1\nline2\nline3").unwrap();

        let out = read_file(dir.path(), "lines.txt", Some(2), Some(100))
            .await
            .unwrap();
        assert!(out.content.contains("    2 | line2"));
        assert!(out.content.contains("    3 | line3"));
        assert_eq!(out.lines_included, 2);
    }

    #[tokio::test]
    async fn read_start_beyond_end_returns_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lines.txt"), "line1\nline2").unwrap();

        let out = read_file(dir.path(), "lines.txt", Some(99), None)
            .await
            .unwrap();
        assert!(out.content.contains("file has 2 lines"));
        assert_eq!(out.lines_included, 0);
    }

    #[tokio::test]
    async fn read_single_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lines.txt"), "line1\nline2\nline3").unwrap();

        let out = read_file(dir.path(), "lines.txt", Some(2), Some(2))
            .await
            .unwrap();
        assert!(out.content.starts_with("    2 | line2"));
        assert_eq!(out.lines_included, 1);
    }

    #[tokio::test]
    async fn default_budget_applies_when_no_range_given() {
        let dir = tempfile::tempdir().unwrap();
        // Build a file just past DEFAULT_LINE_BUDGET so the body alone
        // (with line-number prefixes) overflows the 100 KB byte cap and
        // we exercise the truncation path.
        let body: String = (1..=DEFAULT_LINE_BUDGET + 100)
            .map(|i| format!("{}\n", "x".repeat(80) + &format!("_{i}")))
            .collect();
        std::fs::write(dir.path().join("big.txt"), body).unwrap();

        let out = read_file(dir.path(), "big.txt", None, None).await.unwrap();
        assert!(out.truncated);
        assert!(out.lines_included <= DEFAULT_LINE_BUDGET);
        assert!(out.content.contains("showing lines 1-"));
        assert!(
            out.content
                .contains("pass start_line/end_line to read more")
        );
        // Content is line-numbered.
        assert!(out.content.starts_with("    1 | "));
    }

    #[tokio::test]
    async fn output_byte_cap_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        // ~150 KB file, well under MAX_FILE_SIZE but above OUTPUT_BYTE_BUDGET.
        let line = "y".repeat(200);
        let body: String = (0..1000).map(|_| format!("{line}\n")).collect();
        std::fs::write(dir.path().join("medium.txt"), body).unwrap();

        let out = read_file(dir.path(), "medium.txt", Some(1), Some(1000))
            .await
            .unwrap();
        assert!(out.truncated);
        // Body is capped at OUTPUT_BYTE_BUDGET pre-numbering; line-number
        // prefixes (~8 bytes per line) plus the footer push the final
        // string a bit past that. Keep the slack small but real.
        assert!(out.content.len() < OUTPUT_BYTE_BUDGET * 2);
        assert!(out.lines_included < 1000);
    }

    impl ReadFileOutput {
        /// Test helper — true when the output isn't clipped at all.
        fn fits_for_test(&self) -> bool {
            !self.truncated
        }
    }
}
