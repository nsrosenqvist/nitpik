//! Full file content loader for changed files.

use std::path::Path;

use indexmap::IndexMap;

use crate::models::diff::FileDiff;

/// Number of context lines around each hunk for large-file fallback.
const LARGE_FILE_CONTEXT_LINES: usize = 50;

/// Load full file contents for all changed files.
///
/// For files under `max_lines`, the entire file is loaded.
/// For larger files, extracts the hunk regions plus surrounding context
/// lines so the LLM still has type/function context around the changes.
pub async fn load_file_contents(
    repo_root: &Path,
    diffs: &[FileDiff],
    max_lines: usize,
) -> IndexMap<String, String> {
    let mut contents = IndexMap::new();

    for diff in diffs {
        if diff.is_deleted || diff.is_binary {
            continue;
        }

        let file_path = repo_root.join(diff.path());
        if !file_path.exists() {
            continue;
        }

        match tokio::fs::read_to_string(&file_path).await {
            Ok(content) => {
                let line_count = content.lines().count();
                if line_count <= max_lines {
                    contents.insert(diff.path().to_string(), content);
                } else {
                    // Large file: extract hunk regions + surrounding context
                    let excerpt = extract_hunk_context(&content, diff, LARGE_FILE_CONTEXT_LINES);
                    contents.insert(diff.path().to_string(), excerpt);
                }
            }
            Err(_) => {
                // Skip files that can't be read (binary, permissions, etc.)
                continue;
            }
        }
    }

    contents
}

/// Extract regions around each hunk from a large file.
///
/// For each hunk, takes `context` lines before the hunk start and
/// `context` lines after the hunk end, then joins non-overlapping
/// regions with `[... N lines omitted ...]` markers.
fn extract_hunk_context(content: &str, diff: &FileDiff, context: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    if total == 0 || diff.hunks.is_empty() {
        return content.to_string();
    }

    // Collect ranges (0-indexed, inclusive)
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for hunk in &diff.hunks {
        let start = hunk.new_start.saturating_sub(1) as usize; // 1-indexed to 0-indexed
        let end = start + (hunk.new_count.saturating_sub(1) as usize);

        let range_start = start.saturating_sub(context);
        let range_end = (end + context).min(total.saturating_sub(1));

        ranges.push((range_start, range_end));
    }

    // Merge overlapping ranges
    ranges.sort_by_key(|r| r.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 + 1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    // Build the excerpt
    let mut result = String::new();
    let mut prev_end: Option<usize> = None;

    for (start, end) in &merged {
        if let Some(pe) = prev_end {
            let omitted = start - pe - 1;
            if omitted > 0 {
                result.push_str(&format!("\n[... {} lines omitted ...]\n\n", omitted));
            }
        } else if *start > 0 {
            result.push_str(&format!("[... {} lines omitted ...]\n\n", start));
        }

        for line in lines.iter().take(*end + 1).skip(*start) {
            result.push_str(line);
            result.push('\n');
        }

        prev_end = Some(*end);
    }

    if let Some(pe) = prev_end {
        if pe + 1 < total {
            let remaining = total - pe - 1;
            result.push_str(&format!("\n[... {} lines omitted ...]", remaining));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::{DiffLine, DiffLineType, Hunk};

    #[test]
    fn extract_hunk_context_basic() {
        // 100 lines of content
        let lines: Vec<String> = (1..=100).map(|i| format!("line {i}")).collect();
        let content = lines.join("\n");

        let diff = FileDiff {
            old_path: "test.rs".into(),
            new_path: "test.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 50,
                old_count: 3,
                new_start: 50,
                new_count: 3,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: "new line".into(),
                    old_line_no: None,
                    new_line_no: Some(50),
                }],
            }],
        };

        let result = extract_hunk_context(&content, &diff, 5);

        // Should contain lines around hunk (50±5) but not all 100 lines
        assert!(result.contains("line 45"));
        assert!(result.contains("line 55"));
        assert!(result.contains("[... 44 lines omitted ...]"));
        assert!(!result.contains("line 1\n"));
    }

    #[tokio::test]
    async fn load_small_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("small.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let diffs = vec![FileDiff {
            old_path: "small.rs".into(),
            new_path: "small.rs".into(),
            is_new: true,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![],
        }];

        let contents = load_file_contents(dir.path(), &diffs, 1000).await;
        assert_eq!(contents.len(), 1);
        assert!(contents["small.rs"].contains("fn main()"));
    }

    #[tokio::test]
    async fn skip_deleted_file() {
        let dir = tempfile::tempdir().unwrap();
        let diffs = vec![FileDiff {
            old_path: "deleted.rs".into(),
            new_path: "/dev/null".into(),
            is_new: false,
            is_deleted: true,
            is_rename: false,
            is_binary: false,
            hunks: vec![],
        }];

        let contents = load_file_contents(dir.path(), &diffs, 1000).await;
        assert!(contents.is_empty());
    }

    #[tokio::test]
    async fn skip_binary_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("image.png"), "fake image").unwrap();

        let diffs = vec![FileDiff {
            old_path: "image.png".into(),
            new_path: "image.png".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: true,
            hunks: vec![],
        }];

        let contents = load_file_contents(dir.path(), &diffs, 1000).await;
        assert!(contents.is_empty());
    }

    #[tokio::test]
    async fn skip_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let diffs = vec![FileDiff {
            old_path: "gone.rs".into(),
            new_path: "gone.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![],
        }];

        let contents = load_file_contents(dir.path(), &diffs, 1000).await;
        assert!(contents.is_empty());
    }

    #[tokio::test]
    async fn large_file_uses_hunk_context() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.rs");
        // Create a file with 200 lines
        let lines: Vec<String> = (1..=200).map(|i| format!("line {i}")).collect();
        std::fs::write(&file, lines.join("\n")).unwrap();

        let diffs = vec![FileDiff {
            old_path: "big.rs".into(),
            new_path: "big.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 100,
                old_count: 3,
                new_start: 100,
                new_count: 3,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: "new line".into(),
                    old_line_no: None,
                    new_line_no: Some(100),
                }],
            }],
        }];

        // max_lines = 50, so the 200-line file should trigger excerpt mode
        let contents = load_file_contents(dir.path(), &diffs, 50).await;
        assert_eq!(contents.len(), 1);
        // Should contain context around line 100 but not all 200 lines
        assert!(contents["big.rs"].contains("line 100"));
        assert!(contents["big.rs"].contains("omitted"));
    }
}
