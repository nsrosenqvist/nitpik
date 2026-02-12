//! Diff engine: git CLI wrapper, unified diff parsing, file scanning, and chunk splitting.

pub mod chunker;
pub mod file;
pub mod git;
pub mod parser;
pub mod scanner;

use std::path::Path;
use thiserror::Error;

use crate::models::InputMode;
use crate::models::FileDiff;

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

/// Produce a list of file diffs from the given input mode.
pub async fn get_diffs(input: &InputMode, repo_root: &Path) -> Result<Vec<FileDiff>, DiffError> {
    match input {
        InputMode::DiffFile(path) => {
            let content = file::read_diff_file(path).await?;
            Ok(parser::parse_unified_diff(&content))
        }
        InputMode::Stdin => {
            let content = read_diff_stdin().await?;
            Ok(parser::parse_unified_diff(&content))
        }
        InputMode::GitBase(base_ref) => {
            let diff_output = git::git_diff(repo_root, base_ref).await?;
            Ok(parser::parse_unified_diff(&diff_output))
        }
        InputMode::DirectPath(path) => scanner::scan_path(path).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let diffs = get_diffs(&input, dir.path()).await.unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].new_path, "f.rs");
    }

    #[tokio::test]
    async fn get_diffs_from_direct_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("scan_me.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let input = InputMode::DirectPath(file);
        let diffs = get_diffs(&input, dir.path()).await.unwrap();
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].is_new);
    }

    #[tokio::test]
    async fn get_diffs_file_not_found() {
        let input = InputMode::DiffFile(std::path::PathBuf::from("/tmp/nitpik_nonexistent.diff"));
        let result = get_diffs(&input, Path::new("/tmp")).await;
        assert!(result.is_err());
    }
}
