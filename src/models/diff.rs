//! Diff-related types: file diffs, hunks, and diff lines.

use serde::{Deserialize, Serialize};

/// The type of a line in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineType {
    /// Line exists only in the new version (added).
    Added,
    /// Line exists only in the old version (removed).
    Removed,
    /// Line is unchanged (context).
    Context,
}

/// A single line in a diff hunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    /// The type of change.
    pub line_type: DiffLineType,
    /// The content of the line (without the leading +/-/space).
    pub content: String,
    /// Line number in the old file (None for added lines).
    pub old_line_no: Option<u32>,
    /// Line number in the new file (None for removed lines).
    pub new_line_no: Option<u32>,
}

/// A contiguous hunk within a file diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    /// Starting line in the old file.
    pub old_start: u32,
    /// Number of lines in the old file.
    pub old_count: u32,
    /// Starting line in the new file.
    pub new_start: u32,
    /// Number of lines in the new file.
    pub new_count: u32,
    /// Optional hunk header text (e.g., function name).
    pub header: Option<String>,
    /// The lines in this hunk.
    pub lines: Vec<DiffLine>,
}

/// A diff for a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    /// Path of the old file (may be `/dev/null` for new files).
    pub old_path: String,
    /// Path of the new file (may be `/dev/null` for deleted files).
    pub new_path: String,
    /// Whether this is a new file.
    pub is_new: bool,
    /// Whether this file was deleted.
    pub is_deleted: bool,
    /// Whether this is a rename.
    pub is_rename: bool,
    /// Whether this is a binary file.
    pub is_binary: bool,
    /// The hunks in this diff.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Returns the most relevant file path (new_path for non-deletes, old_path for deletes).
    pub fn path(&self) -> &str {
        if self.is_deleted {
            &self.old_path
        } else {
            &self.new_path
        }
    }

    /// Returns the total number of added lines across all hunks.
    #[allow(dead_code)]
    pub fn added_lines(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.line_type == DiffLineType::Added)
            .count()
    }

    /// Returns the total number of removed lines across all hunks.
    #[allow(dead_code)]
    pub fn removed_lines(&self) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.line_type == DiffLineType::Removed)
            .count()
    }
}
