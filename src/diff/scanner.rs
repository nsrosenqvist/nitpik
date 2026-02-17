//! File and directory scanner for direct review mode.
//!
//! When `--path` is used, we scan files and create synthetic diffs
//! treating all content as "added" for review purposes.

use std::path::Path;

use ignore::WalkBuilder;

use crate::models::diff::{DiffLine, DiffLineType, FileDiff, Hunk};

use super::DiffError;

/// Scan a file or directory and produce synthetic file diffs for review.
pub async fn scan_path(path: &Path) -> Result<Vec<FileDiff>, DiffError> {
    if !path.exists() {
        return Err(DiffError::PathNotFound(path.display().to_string()));
    }

    let mut diffs = Vec::new();

    if path.is_file() {
        if let Some(diff) = scan_single_file(path).await? {
            diffs.push(diff);
        }
    } else if path.is_dir() {
        let walker = WalkBuilder::new(path).hidden(true).git_ignore(true).build();

        for entry in walker.flatten() {
            if entry.file_type().map_or(true, |ft| !ft.is_file()) {
                continue;
            }
            if let Some(diff) = scan_single_file(entry.path()).await? {
                diffs.push(diff);
            }
        }
    }

    Ok(diffs)
}

/// Create a synthetic diff for a single file (all lines as "added").
async fn scan_single_file(path: &Path) -> Result<Option<FileDiff>, DiffError> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return Ok(None), // Skip binary or unreadable files
    };

    if content.is_empty() {
        return Ok(None);
    }

    let lines: Vec<DiffLine> = content
        .lines()
        .enumerate()
        .map(|(i, line)| DiffLine {
            line_type: DiffLineType::Added,
            content: line.to_string(),
            old_line_no: None,
            new_line_no: Some(i as u32 + 1),
        })
        .collect();

    let line_count = lines.len() as u32;
    let path_str = path.display().to_string();

    Ok(Some(FileDiff {
        old_path: "/dev/null".to_string(),
        new_path: path_str,
        is_new: true,
        is_deleted: false,
        is_rename: false,
        is_binary: false,
        hunks: vec![Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: line_count,
            header: None,
            lines,
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::DiffLineType;

    #[tokio::test]
    async fn scan_nonexistent_path() {
        let result = scan_path(Path::new("/tmp/nitpik_does_not_exist_12345")).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn scan_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.rs");
        std::fs::write(&file, "fn main() {\n    println!(\"hi\");\n}\n").unwrap();

        let diffs = scan_path(&file).await.unwrap();
        assert_eq!(diffs.len(), 1);

        let d = &diffs[0];
        assert!(d.is_new);
        assert!(!d.is_deleted);
        assert_eq!(d.old_path, "/dev/null");
        assert!(d.new_path.contains("hello.rs"));
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.hunks[0].new_start, 1);
        assert_eq!(d.hunks[0].new_count, 3);
        for line in &d.hunks[0].lines {
            assert_eq!(line.line_type, DiffLineType::Added);
        }
    }

    #[tokio::test]
    async fn scan_empty_file_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("empty.txt");
        std::fs::write(&file, "").unwrap();

        let diffs = scan_path(&file).await.unwrap();
        assert!(diffs.is_empty(), "empty file should produce no diff");
    }

    #[tokio::test]
    async fn scan_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "line1\nline2").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello").unwrap();
        // Empty file should be skipped
        std::fs::write(dir.path().join("c.txt"), "").unwrap();

        let diffs = scan_path(dir.path()).await.unwrap();
        // a.txt and b.txt should produce diffs; c.txt is empty → skipped
        assert_eq!(diffs.len(), 2);
    }

    #[tokio::test]
    async fn scan_directory_respects_hidden() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("visible.txt"), "content").unwrap();
        std::fs::create_dir(dir.path().join(".hidden_dir")).unwrap();
        std::fs::write(dir.path().join(".hidden_dir").join("secret.txt"), "secret").unwrap();

        let diffs = scan_path(dir.path()).await.unwrap();
        // Only visible.txt should appear; .hidden_dir is skipped
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].new_path.contains("visible.txt"));
    }

    #[tokio::test]
    async fn scan_binary_file_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("binary.bin");
        // Write invalid UTF-8
        std::fs::write(&file, &[0xFF, 0xFE, 0x00, 0x01]).unwrap();

        let diffs = scan_path(&file).await.unwrap();
        assert!(diffs.is_empty(), "binary file should be skipped");
    }
}
