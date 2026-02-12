//! Auto-profile selection via cheap LLM call.
//!
//! When `--profile auto` is used, we analyze the changed file extensions
//! and paths to select appropriate built-in profiles without an LLM call.

use crate::models::diff::FileDiff;
use crate::models::DEFAULT_PROFILE;

/// Select appropriate profiles based on changed files.
///
/// Uses file extension and path heuristics to choose built-in profiles.
/// Falls back to the default profile if no specific match is found.
pub fn auto_select_profiles(diffs: &[FileDiff]) -> Vec<String> {
    let mut profiles = Vec::new();
    let mut has_frontend = false;
    let mut has_backend = false;

    for diff in diffs {
        let path = diff.path();
        let ext = path.rsplit('.').next().unwrap_or("");

        match ext {
            "js" | "jsx" | "ts" | "tsx" | "vue" | "svelte" | "css" | "scss" | "html" => {
                has_frontend = true;
            }
            "rs" | "go" | "py" | "rb" | "java" | "kt" | "cs" | "php" | "ex" | "exs" => {
                has_backend = true;
            }
            _ => {}
        }

        // Path-based heuristics
        if path.contains("frontend/")
            || path.contains("client/")
            || path.contains("components/")
            || path.contains("pages/")
        {
            has_frontend = true;
        }
        if path.contains("backend/")
            || path.contains("server/")
            || path.contains("api/")
            || path.contains("src/")
        {
            has_backend = true;
        }
    }

    if has_frontend {
        profiles.push("frontend".to_string());
    }
    if has_backend || (!has_frontend && !has_backend) {
        profiles.push(DEFAULT_PROFILE.to_string());
    }

    // Always include security for comprehensive reviews
    profiles.push("security".to_string());

    profiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::FileDiff;

    fn make_diff(path: &str) -> FileDiff {
        FileDiff {
            old_path: path.to_string(),
            new_path: path.to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![],
        }
    }

    #[test]
    fn auto_selects_frontend() {
        let diffs = vec![make_diff("src/components/Button.tsx")];
        let profiles = auto_select_profiles(&diffs);
        assert!(profiles.contains(&"frontend".to_string()));
    }

    #[test]
    fn auto_selects_backend() {
        let diffs = vec![make_diff("src/handler.rs")];
        let profiles = auto_select_profiles(&diffs);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn auto_defaults_to_backend() {
        let diffs = vec![make_diff("README.md")];
        let profiles = auto_select_profiles(&diffs);
        assert!(profiles.contains(&"backend".to_string()));
    }

    #[test]
    fn always_includes_security() {
        let diffs = vec![make_diff("anything.txt")];
        let profiles = auto_select_profiles(&diffs);
        assert!(profiles.contains(&"security".to_string()));
    }
}
