//! Chunk splitter for large diffs.
//!
//! Splits large file diffs into smaller chunks at heuristic boundaries
//! (blank lines, function boundaries) to fit within LLM context limits.

use crate::models::diff::FileDiff;

/// Default max lines per chunk.
const DEFAULT_MAX_CHUNK_LINES: usize = 500;

/// Split a file diff into smaller chunks if it exceeds the line limit.
///
/// Each chunk is a subset of hunks from the original diff. Returns the
/// original diff in a single-element vec if it's small enough.
pub fn chunk_diff(diff: &FileDiff, max_lines: Option<usize>) -> Vec<FileDiff> {
    let max = max_lines.unwrap_or(DEFAULT_MAX_CHUNK_LINES);

    let total_lines: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
    if total_lines <= max {
        return vec![diff.clone()];
    }

    // Split by hunks first — each hunk becomes its own chunk
    // If a single hunk is too large, we keep it as-is (LLM can handle it)
    let mut chunks: Vec<FileDiff> = Vec::new();
    let mut current_hunks = Vec::new();
    let mut current_lines = 0;

    for hunk in &diff.hunks {
        let hunk_lines = hunk.lines.len();

        if current_lines + hunk_lines > max && !current_hunks.is_empty() {
            // Flush current chunk
            chunks.push(FileDiff {
                old_path: diff.old_path.clone(),
                new_path: diff.new_path.clone(),
                is_new: diff.is_new,
                is_deleted: diff.is_deleted,
                is_rename: diff.is_rename,
                is_binary: diff.is_binary,
                hunks: std::mem::take(&mut current_hunks),
            });
            current_lines = 0;
        }

        current_hunks.push(hunk.clone());
        current_lines += hunk_lines;
    }

    if !current_hunks.is_empty() {
        chunks.push(FileDiff {
            old_path: diff.old_path.clone(),
            new_path: diff.new_path.clone(),
            is_new: diff.is_new,
            is_deleted: diff.is_deleted,
            is_rename: diff.is_rename,
            is_binary: diff.is_binary,
            hunks: current_hunks,
        });
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::{DiffLine, DiffLineType, Hunk};

    fn make_hunk(line_count: usize) -> Hunk {
        Hunk {
            old_start: 1,
            old_count: line_count as u32,
            new_start: 1,
            new_count: line_count as u32,
            header: None,
            lines: (0..line_count)
                .map(|i| DiffLine {
                    line_type: DiffLineType::Context,
                    content: format!("line {i}"),
                    old_line_no: Some(i as u32 + 1),
                    new_line_no: Some(i as u32 + 1),
                })
                .collect(),
        }
    }

    #[test]
    fn small_diff_not_chunked() {
        let diff = FileDiff {
            old_path: "a.rs".into(),
            new_path: "a.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![make_hunk(10)],
        };
        let chunks = chunk_diff(&diff, Some(500));
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn large_diff_chunked_by_hunks() {
        let diff = FileDiff {
            old_path: "a.rs".into(),
            new_path: "a.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![make_hunk(100), make_hunk(100), make_hunk(100)],
        };
        let chunks = chunk_diff(&diff, Some(150));
        assert_eq!(chunks.len(), 3);
        for chunk in &chunks {
            assert_eq!(chunk.hunks.len(), 1);
        }
    }
}
