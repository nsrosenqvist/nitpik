//! Diff engine: git CLI wrapper, unified diff parsing, file scanning, and chunk splitting.
//!
//! # Bounded Context: Diff Retrieval & Parsing
//!
//! Owns git invocation, unified-diff parsing, directory scanning,
//! and chunk splitting. Produces [`FileDiff`](crate::models::diff::FileDiff)
//! values — never interprets diff content semantically.

pub mod chunker;
pub mod file;
pub mod git;
pub mod parser;
pub mod scanner;

use std::path::Path;
use thiserror::Error;

use crate::models::InputMode;
use crate::models::diff::FileDiff;

/// Errors from the diff engine.
#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum DiffError {
    #[error("git command failed: {0}")]
    GitError(String),

    #[error("failed to read diff file: {0}")]
    FileReadError(#[from] std::io::Error),

    #[error("diff parse error: {0}")]
    ParseError(String),

    #[error("path not found: {0}")]
    PathNotFound(String),
}

/// Read a unified diff from stdin.
pub async fn read_diff_stdin() -> Result<String, DiffError> {
    use tokio::io::AsyncReadExt;
    let mut buf = String::new();
    tokio::io::stdin()
        .read_to_string(&mut buf)
        .await
        .map_err(DiffError::FileReadError)?;
    Ok(buf)
}

/// The raw diff source: either a string to parse (zero-copy-friendly)
/// or already-parsed owned diffs from a directory scan.
pub enum DiffSource {
    /// Raw unified diff text — call [`parser::parse_unified_diff`] to
    /// get `FileDiff` values that borrow from this string.
    Raw(String),
    /// Pre-parsed diffs from [`scanner::scan_path`] (all owned).
    Scanned(Vec<FileDiff<'static>>),
}

/// Obtain the diff source for the given input mode.
///
/// For git/file/stdin modes this returns the raw diff string so the
/// caller can parse it in a scope where the string lives long enough
/// to be borrowed (zero-copy).  For direct-path scans the diffs are
/// returned pre-parsed with owned content.
pub async fn get_diff_source(input: &InputMode, repo_root: &Path) -> Result<DiffSource, DiffError> {
    match input {
        InputMode::DiffFile(path) => Ok(DiffSource::Raw(file::read_diff_file(path).await?)),
        InputMode::Stdin => Ok(DiffSource::Raw(read_diff_stdin().await?)),
        InputMode::GitBase(base_ref) => {
            Ok(DiffSource::Raw(git::git_diff(repo_root, base_ref).await?))
        }
        InputMode::DirectPath(path) => Ok(DiffSource::Scanned(scanner::scan_path(path).await?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: resolve a DiffSource into a Vec of owned diffs.
    async fn resolve_diffs(
        input: &InputMode,
        root: &Path,
    ) -> Result<Vec<FileDiff<'static>>, DiffError> {
        let source = get_diff_source(input, root).await?;
        match source {
            DiffSource::Raw(content) => Ok(parser::parse_unified_diff(&content)
                .into_iter()
                .map(|d| {
                    // Convert borrowed content to owned for test assertions
                    FileDiff {
                        old_path: d.old_path,
                        new_path: d.new_path,
                        is_new: d.is_new,
                        is_deleted: d.is_deleted,
                        is_rename: d.is_rename,
                        is_binary: d.is_binary,
                        hunks: d
                            .hunks
                            .into_iter()
                            .map(|h| crate::models::diff::Hunk {
                                old_start: h.old_start,
                                old_count: h.old_count,
                                new_start: h.new_start,
                                new_count: h.new_count,
                                header: h.header,
                                lines: h
                                    .lines
                                    .into_iter()
                                    .map(|l| crate::models::diff::DiffLine {
                                        line_type: l.line_type,
                                        content: std::borrow::Cow::Owned(l.content.into_owned()),
                                        old_line_no: l.old_line_no,
                                        new_line_no: l.new_line_no,
                                    })
                                    .collect(),
                            })
                            .collect(),
                    }
                })
                .collect()),
            DiffSource::Scanned(diffs) => Ok(diffs),
        }
    }

    #[tokio::test]
    async fn get_diffs_from_diff_file() {
        let dir = tempfile::tempdir().unwrap();
        let diff_path = dir.path().join("test.diff");
        std::fs::write(
            &diff_path,
            "diff --git a/f.rs b/f.rs\nindex 111..222 100644\n--- a/f.rs\n+++ b/f.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n",
        )
        .unwrap();

        let input = InputMode::DiffFile(diff_path);
        let diffs = resolve_diffs(&input, dir.path()).await.unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].new_path, "f.rs");
    }

    #[tokio::test]
    async fn get_diffs_from_direct_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan_me.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let input = InputMode::DirectPath(file);
        let diffs = resolve_diffs(&input, dir.path()).await.unwrap();
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].is_new);
    }

    #[tokio::test]
    async fn get_diffs_file_not_found() {
        let input = InputMode::DiffFile(std::path::PathBuf::from("/tmp/nitpik_nonexistent.diff"));
        let result = resolve_diffs(&input, Path::new("/tmp")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_diffs_git_base_in_real_repo() {
        // This test runs in the actual nitpik git repo.
        // Diffing HEAD against itself should produce an empty diff.
        let repo_root = git::find_repo_root(Path::new(env!("CARGO_MANIFEST_DIR")))
            .await
            .expect("should find git repo root");
        let input = InputMode::GitBase("HEAD".to_string());
        let diffs = resolve_diffs(&input, Path::new(&repo_root)).await.unwrap();
        // HEAD vs HEAD = empty diff (may be non-empty if working tree is dirty,
        // but the call itself must succeed).
        let _ = diffs;
    }

    #[tokio::test]
    async fn get_diffs_git_base_in_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let input = InputMode::GitBase("HEAD".to_string());
        let result = resolve_diffs(&input, dir.path()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_diffs_direct_path_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() {}\n").unwrap();
        let input = InputMode::DirectPath(dir.path().to_path_buf());
        let diffs = resolve_diffs(&input, dir.path()).await.unwrap();
        assert!(diffs.len() >= 2);
    }
}
