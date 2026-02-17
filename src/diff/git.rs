//! Git CLI wrapper for producing diffs.
//!
//! Shells out to `git` via `tokio::process::Command`.

use std::path::Path;

use super::DiffError;
use crate::env::Env;

/// Run `git diff <base_ref>` and return the unified diff output.
pub async fn git_diff(repo_root: &Path, base_ref: &str) -> Result<String, DiffError> {
    let output = tokio::process::Command::new("git")
        .args(["diff", "--src-prefix=a/", "--dst-prefix=b/", base_ref])
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| DiffError::GitError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DiffError::GitError(format!(
            "git diff failed (exit {}): {stderr}",
            output.status
        )));
    }

    String::from_utf8(output.stdout)
        .map_err(|e| DiffError::GitError(format!("git output is not valid UTF-8: {e}")))
}

/// Detect the current branch or review scope.
///
/// Tries, in order:
/// 1. `git rev-parse --abbrev-ref HEAD` (returns branch name, or `HEAD` when detached)
/// 2. CI-specific environment variables (`GITHUB_HEAD_REF`, `CI_COMMIT_BRANCH`,
///    `BITBUCKET_BRANCH`, `CI_BRANCH`)
/// 3. Returns an empty string when nothing is available.
///
/// The result is used to scope sidecar `.meta` files so that parallel
/// PRs/branches reviewing the same file don't cross-contaminate prior findings.
pub async fn detect_branch(repo_root: &Path, env: &Env) -> String {
    // Try git first
    if let Ok(output) = tokio::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_root)
        .output()
        .await
    {
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // "HEAD" means detached — fall through to CI env vars
            if !branch.is_empty() && branch != "HEAD" {
                return branch;
            }
        }
    }

    // CI env var fallback (detached HEAD is common in CI)
    for var in &[
        "GITHUB_HEAD_REF",
        "CI_COMMIT_BRANCH",
        "CI_MERGE_REQUEST_SOURCE_BRANCH_NAME",
        "BITBUCKET_BRANCH",
        "CI_BRANCH",
    ] {
        if let Ok(val) = env.var(var) {
            let val = val.trim().to_string();
            if !val.is_empty() {
                return val;
            }
        }
    }

    String::new()
}

/// Find the root of the git repository containing `start_dir`.
pub async fn find_repo_root(start_dir: &Path) -> Result<String, DiffError> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start_dir)
        .output()
        .await
        .map_err(|e| DiffError::GitError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DiffError::GitError(format!(
            "not a git repository: {stderr}"
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Env;

    #[tokio::test]
    async fn git_diff_in_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = git_diff(dir.path(), "HEAD").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("git diff failed") || err.contains("git"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn find_repo_root_non_git() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_repo_root(dir.path()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not a git repository"), "got: {err}");
    }

    #[tokio::test]
    async fn git_diff_in_real_repo() {
        // Create a temp git repo with a commit so HEAD exists
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();

        // Initialize repo and make a commit
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::fs::write(p.join("file.txt"), "hello\n").await.unwrap();
        tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(p)
            .output()
            .await
            .unwrap();

        // Now modify a file for a non-empty diff
        tokio::fs::write(p.join("file.txt"), "hello\nworld\n").await.unwrap();

        let result = git_diff(p, "HEAD").await;
        assert!(result.is_ok(), "git diff failed: {:?}", result.unwrap_err());
        let diff = result.unwrap();
        assert!(diff.contains("world"), "diff should contain the change");
    }

    #[tokio::test]
    async fn find_repo_root_real() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = find_repo_root(repo).await.unwrap();
        assert!(!root.is_empty());
    }

    #[tokio::test]
    async fn detect_branch_returns_branch_name_in_git_repo() {
        // Create a temp git repo on a named branch
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        tokio::process::Command::new("git")
            .args(["init", "-b", "test-branch"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::fs::write(p.join("file.txt"), "hello\n").await.unwrap();
        tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(p)
            .output()
            .await
            .unwrap();

        let branch = detect_branch(p, &Env::real()).await;
        assert_eq!(branch, "test-branch");
    }

    #[tokio::test]
    async fn detect_branch_returns_empty_for_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let branch = detect_branch(dir.path(), &Env::real()).await;
        assert!(branch.is_empty(), "non-git dir should return empty, got: {branch}");
    }

    #[tokio::test]
    async fn detect_branch_falls_back_to_ci_env_var() {
        // Create a git repo with detached HEAD
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::fs::write(p.join("file.txt"), "hello\n").await.unwrap();
        tokio::process::Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        tokio::process::Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(p)
            .output()
            .await
            .unwrap();
        // Detach HEAD
        tokio::process::Command::new("git")
            .args(["checkout", "--detach"])
            .current_dir(p)
            .output()
            .await
            .unwrap();

        // Set a CI env var via mock
        let env = Env::mock([("GITHUB_HEAD_REF", "pr-42-branch")]);
        let branch = detect_branch(p, &env).await;

        assert_eq!(branch, "pr-42-branch");
    }
}
