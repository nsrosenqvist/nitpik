//! Baseline context assembly.
//!
//! Orchestrates loading full file contents for changed files
//! and auto-detecting project documentation files.

pub mod files;
pub mod project_docs;

use std::path::Path;

use indexmap::IndexMap;

use crate::config::Config;
use crate::models::context::BaselineContext;
use crate::models::diff::FileDiff;

/// Build the baseline context for a review.
///
/// Loads full file contents for all changed files and discovers
/// project documentation files in the repository root.
///
/// When `skip_project_docs` is true, no project docs are included.
/// Otherwise, `exclude_docs` can filter out specific filenames.
///
/// `commit_log` is passed through as-is — the caller is responsible for
/// gathering it (via `git_log`) when the input mode is a git ref diff.
pub async fn build_baseline_context(
    repo_root: &Path,
    diffs: &[FileDiff],
    config: &Config,
    skip_project_docs: bool,
    exclude_docs: &[String],
    commit_log: Vec<String>,
) -> BaselineContext {
    let file_contents =
        files::load_file_contents(repo_root, diffs, config.review.context.max_file_lines).await;

    let project_docs = if skip_project_docs {
        IndexMap::new()
    } else {
        project_docs::detect_project_docs(repo_root, exclude_docs).await
    };

    BaselineContext {
        file_contents,
        project_docs,
        commit_log,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::{FileDiff, Hunk};

    fn make_diff(path: &str) -> FileDiff {
        FileDiff {
            old_path: path.to_string(),
            new_path: path.to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                header: None,
                lines: vec![],
            }],
        }
    }

    #[tokio::test]
    async fn build_context_loads_files_and_docs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Guide").unwrap();

        let diffs = vec![make_diff("main.rs")];
        let config = Config::default();

        let ctx = build_baseline_context(dir.path(), &diffs, &config, false, &[], Vec::new()).await;
        assert!(ctx.file_contents.contains_key("main.rs"));
        assert_eq!(ctx.project_docs.len(), 1);
        assert!(ctx.project_docs.contains_key("AGENTS.md"));
    }

    #[tokio::test]
    async fn build_context_empty_diffs() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::default();

        let ctx = build_baseline_context(dir.path(), &[], &config, false, &[], Vec::new()).await;
    }

    #[tokio::test]
    async fn build_context_skips_deleted_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("deleted.rs"), "old content").unwrap();

        let mut diff = make_diff("deleted.rs");
        diff.is_deleted = true;
        let config = Config::default();

        let ctx =
            build_baseline_context(dir.path(), &[diff], &config, false, &[], Vec::new()).await;
        assert!(ctx.file_contents.is_empty());
    }

    #[tokio::test]
    async fn build_context_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        // Diff references a file that doesn't exist on disk.
        let diffs = vec![make_diff("nonexistent.rs")];
        let config = Config::default();

        let ctx = build_baseline_context(dir.path(), &diffs, &config, false, &[], Vec::new()).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Guide").unwrap();
        std::fs::write(dir.path().join("CONVENTIONS.md"), "# Rules").unwrap();

        let diffs = vec![make_diff("main.rs")];
        let config = Config::default();

        let ctx = build_baseline_context(dir.path(), &diffs, &config, true, &[], Vec::new()).await;
    }

    #[tokio::test]
    async fn build_context_exclude_specific_doc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Guide").unwrap();
        std::fs::write(dir.path().join("CONVENTIONS.md"), "# Rules").unwrap();

        let diffs = vec![make_diff("main.rs")];
        let config = Config::default();
        let exclude = vec!["AGENTS.md".to_string()];

        let ctx =
            build_baseline_context(dir.path(), &diffs, &config, false, &exclude, Vec::new()).await;
        assert_eq!(ctx.file_contents.len(), 1);
        assert_eq!(ctx.project_docs.len(), 1);
        assert!(!ctx.project_docs.contains_key("AGENTS.md"));
        assert!(ctx.project_docs.contains_key("CONVENTIONS.md"));
    }

    #[tokio::test]
    async fn build_context_skip_overrides_exclude() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Guide").unwrap();

        let config = Config::default();
        // Even with an empty exclude list, skip_project_docs=true wins
        let ctx = build_baseline_context(dir.path(), &[], &config, true, &[], Vec::new()).await;
        assert!(ctx.project_docs.is_empty());
    }
}
