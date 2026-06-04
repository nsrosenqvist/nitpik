//! Review context types.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::diff::FileDiff;

/// Baseline context assembled before sending to the LLM.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BaselineContext {
    /// Full file contents for changed files (path → content, insertion-ordered).
    pub file_contents: IndexMap<String, String>,
    /// Project documentation files found (path → content, insertion-ordered).
    pub project_docs: IndexMap<String, String>,
    /// Commit summaries between the diff base and HEAD (reverse chronological).
    /// Empty when the input is not a git ref diff (e.g. stdin, file, scan).
    pub commit_log: Vec<String>,
    /// Rolling functional PR summary carried across runs (what the PR does,
    /// subsystems touched, open risks). `None` unless the rolling-summary
    /// feature is enabled and a summary was generated or restored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_summary: Option<String>,
    /// The PR author's stated intent — the pull request title and description
    /// as written by the author. Sourced from the CI event payload (or an
    /// explicit override). `None` outside a PR context or when disabled.
    /// Treated as a description to verify against the diff, never as
    /// instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_intent: Option<String>,
}

/// The complete context for a single review request.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ReviewContext<'a> {
    /// The file diffs to review.
    pub diffs: Vec<FileDiff<'a>>,
    /// Pre-assembled baseline context.
    pub baseline: BaselineContext,
    /// The root path of the repository.
    pub repo_root: String,
    /// Whether this is a path-based scan (no diff boundaries to enforce).
    pub is_path_scan: bool,
}
