//! Auto-detect project documentation files.
//!
//! Looks for well-known documentation files in the repo root
//! that provide context for code reviews.
//!
//! **Priority files** (`REVIEW.md`, `NITPIK.md`) take precedence: when any of
//! them exist (and are not excluded), *only* those are included and the generic
//! doc list is skipped entirely. This lets maintainers provide focused review
//! guidance without polluting the prompt with coding-agent instructions.

use std::path::Path;

use indexmap::IndexMap;

use crate::constants::PRIORITY_DOC_FILES;

/// Well-known project documentation filenames (generic fallback list).
const PROJECT_DOC_FILES: &[&str] = &[
    "AGENTS.md",
    "ARCHITECTURE.md",
    "CONVENTIONS.md",
    "CONTRIBUTING.md",
    "CLAUDE.md",
    ".github/copilot-instructions.md",
    ".cursorrules",
    "CODING_GUIDELINES.md",
    "STYLE_GUIDE.md",
    "DEVELOPMENT.md",
];

/// Maximum size of a project doc to include (256 KB).
const MAX_DOC_SIZE: u64 = 256 * 1024;

/// Detect and load project documentation files from the repo root.
///
/// **Precedence rule:** if any priority review context file (`REVIEW.md` or
/// `NITPIK.md`) exists and is not excluded, only the priority files are
/// returned. Otherwise, the generic doc list is scanned as before.
///
/// Pass `exclude` to skip specific filenames (e.g. `["AGENTS.md"]`).
/// The names are matched exactly against the filename lists.
pub async fn detect_project_docs(
    repo_root: &Path,
    exclude: &[String],
) -> IndexMap<String, String> {
    // First pass: check for priority review context files.
    let priority = load_doc_list(repo_root, PRIORITY_DOC_FILES, exclude).await;
    if !priority.is_empty() {
        return priority;
    }

    // Fallback: scan the generic project doc list.
    load_doc_list(repo_root, PROJECT_DOC_FILES, exclude).await
}

/// Load docs from `candidates` that exist on disk, are not excluded,
/// and are within the size limit.
async fn load_doc_list(
    repo_root: &Path,
    candidates: &[&str],
    exclude: &[String],
) -> IndexMap<String, String> {
    let mut docs = IndexMap::new();

    for &filename in candidates {
        if exclude.iter().any(|e| e == filename) {
            continue;
        }

        let path = repo_root.join(filename);
        if !path.exists() {
            continue;
        }

        // Check file size before reading
        if let Ok(metadata) = tokio::fs::metadata(&path).await {
            if metadata.len() > MAX_DOC_SIZE {
                continue;
            }
        }

        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            docs.insert(filename.to_string(), content);
        }
    }

    docs
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fallback behavior (no priority files) ───────────────────────

    #[tokio::test]
    async fn detect_docs_in_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let docs = detect_project_docs(dir.path(), &[]).await;
        assert!(docs.is_empty());
    }

    #[tokio::test]
    async fn detect_agents_md_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();
        let docs = detect_project_docs(dir.path(), &[]).await;
        assert_eq!(docs.len(), 1);
        assert_eq!(docs["AGENTS.md"], "# Agent Guide");
    }

    #[tokio::test]
    async fn exclude_specific_doc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();
        std::fs::write(dir.path().join("CONVENTIONS.md"), "# Conventions").unwrap();

        let exclude = vec!["AGENTS.md".to_string()];
        let docs = detect_project_docs(dir.path(), &exclude).await;
        assert_eq!(docs.len(), 1);
        assert!(!docs.contains_key("AGENTS.md"));
        assert!(docs.contains_key("CONVENTIONS.md"));
    }

    #[tokio::test]
    async fn exclude_multiple_docs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agents").unwrap();
        std::fs::write(dir.path().join("CONVENTIONS.md"), "# Conventions").unwrap();
        std::fs::write(dir.path().join("CONTRIBUTING.md"), "# Contributing").unwrap();

        let exclude = vec!["AGENTS.md".to_string(), "CONTRIBUTING.md".to_string()];
        let docs = detect_project_docs(dir.path(), &exclude).await;
        assert_eq!(docs.len(), 1);
        assert!(docs.contains_key("CONVENTIONS.md"));
    }

    #[tokio::test]
    async fn exclude_nonexistent_doc_is_harmless() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Guide").unwrap();

        let exclude = vec!["NONEXISTENT.md".to_string()];
        let docs = detect_project_docs(dir.path(), &exclude).await;
        assert_eq!(docs.len(), 1);
        assert!(docs.contains_key("AGENTS.md"));
    }

    // ── Priority file precedence ────────────────────────────────────

    #[tokio::test]
    async fn review_md_takes_precedence_over_generic_docs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("REVIEW.md"), "# Review Rules").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();
        std::fs::write(dir.path().join("CONVENTIONS.md"), "# Conventions").unwrap();

        let docs = detect_project_docs(dir.path(), &[]).await;
        assert_eq!(docs.len(), 1);
        assert_eq!(docs["REVIEW.md"], "# Review Rules");
        assert!(!docs.contains_key("AGENTS.md"));
        assert!(!docs.contains_key("CONVENTIONS.md"));
    }

    #[tokio::test]
    async fn nitpik_md_takes_precedence_over_generic_docs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("NITPIK.md"), "# Nitpik Review").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();

        let docs = detect_project_docs(dir.path(), &[]).await;
        assert_eq!(docs.len(), 1);
        assert_eq!(docs["NITPIK.md"], "# Nitpik Review");
        assert!(!docs.contains_key("AGENTS.md"));
    }

    #[tokio::test]
    async fn both_priority_files_included_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("REVIEW.md"), "# Review").unwrap();
        std::fs::write(dir.path().join("NITPIK.md"), "# Nitpik").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();

        let docs = detect_project_docs(dir.path(), &[]).await;
        assert_eq!(docs.len(), 2);
        assert!(docs.contains_key("REVIEW.md"));
        assert!(docs.contains_key("NITPIK.md"));
        assert!(!docs.contains_key("AGENTS.md"));
    }

    #[tokio::test]
    async fn excluding_all_priority_files_falls_back_to_generic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("REVIEW.md"), "# Review").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();

        let exclude = vec!["REVIEW.md".to_string()];
        let docs = detect_project_docs(dir.path(), &exclude).await;
        // REVIEW.md is excluded, so we fall back to the generic list
        assert_eq!(docs.len(), 1);
        assert!(docs.contains_key("AGENTS.md"));
        assert!(!docs.contains_key("REVIEW.md"));
    }

    #[tokio::test]
    async fn excluding_one_priority_file_still_uses_priority_if_other_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("REVIEW.md"), "# Review").unwrap();
        std::fs::write(dir.path().join("NITPIK.md"), "# Nitpik").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();

        let exclude = vec!["REVIEW.md".to_string()];
        let docs = detect_project_docs(dir.path(), &exclude).await;
        // NITPIK.md still matches as priority, so generic docs are skipped
        assert_eq!(docs.len(), 1);
        assert!(docs.contains_key("NITPIK.md"));
        assert!(!docs.contains_key("REVIEW.md"));
        assert!(!docs.contains_key("AGENTS.md"));
    }

    #[tokio::test]
    async fn priority_file_excluded_alongside_generic_exclusion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("REVIEW.md"), "# Review").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Agent Guide").unwrap();
        std::fs::write(dir.path().join("CONVENTIONS.md"), "# Conventions").unwrap();

        // Exclude the only priority file AND a generic one
        let exclude = vec!["REVIEW.md".to_string(), "AGENTS.md".to_string()];
        let docs = detect_project_docs(dir.path(), &exclude).await;
        // Falls back to generic, minus AGENTS.md
        assert_eq!(docs.len(), 1);
        assert!(docs.contains_key("CONVENTIONS.md"));
    }
}
