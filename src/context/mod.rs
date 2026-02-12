//! Baseline context assembly.
//!
//! Orchestrates loading full file contents for changed files
//! and auto-detecting project documentation files.

pub mod files;
pub mod project_docs;

use std::path::Path;

use crate::config::Config;
use crate::models::context::BaselineContext;
use crate::models::diff::FileDiff;

/// Build the baseline context for a review.
///
/// Loads full file contents for all changed files and discovers
/// project documentation files in the repository root.
pub async fn build_baseline_context(
    repo_root: &Path,
    diffs: &[FileDiff],
    config: &Config,
) -> BaselineContext {
    let file_contents = files::load_file_contents(
        repo_root,
        diffs,
        config.review.context.max_file_lines,
    )
    .await;

    let project_docs = project_docs::detect_project_docs(repo_root).await;

    BaselineContext {
        file_contents,
        project_docs,
    }
}
