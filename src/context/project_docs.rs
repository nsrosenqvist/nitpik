//! Auto-detect project documentation files.
//!
//! Looks for well-known documentation files in the repo root
//! that provide context for code reviews.

use std::path::Path;

use indexmap::IndexMap;

/// Well-known project documentation filenames.
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

/// Maximum size of a project doc to include (256KB).
const MAX_DOC_SIZE: u64 = 256 * 1024;

/// Detect and load project documentation files from the repo root.
///
/// Pass `exclude` to skip specific filenames (e.g. `["AGENTS.md"]`).
/// The names are matched exactly against the well-known filename list.
pub async fn detect_project_docs(
    repo_root: &Path,
    exclude: &[String],
) -> IndexMap<String, String> {
    let mut docs = IndexMap::new();

    for &filename in PROJECT_DOC_FILES {
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

    #[tokio::test]
    async fn detect_docs_in_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let docs = detect_project_docs(dir.path(), &[]).await;
        assert!(docs.is_empty());
    }

    #[tokio::test]
    async fn detect_agents_md() {
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
}
