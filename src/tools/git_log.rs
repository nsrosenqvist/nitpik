//! GitLogTool — show the recent commit history of a file in the repository.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.
//!
//! This is the reviewer's window into *why* code is the way it is. The
//! diff alone shows what changed; the history shows whether a change
//! re-introduces a bug a prior commit fixed, reverts a deliberate
//! decision, or contradicts the intent recorded in earlier messages —
//! a class of regression that is invisible from the diff in isolation.
//!
//! History is followed across renames (`--follow`) and capped so a
//! long-lived file can't flood the model's context. Line-level blame
//! (`git log -L`) is intentionally out of scope for now: its output
//! shape varies across git versions, whereas file-level `--follow`
//! history is stable and covers the common "does this undo a past fix?"
//! question.

use std::path::{Path, PathBuf};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Default number of commits returned when the caller omits `max_count`.
const DEFAULT_MAX_COUNT: usize = 10;

/// Hard ceiling on commits returned, regardless of `max_count`. Keeps a
/// churn-heavy file's history from dominating the context window.
const MAX_COUNT_CEILING: usize = 30;

/// Maximum length of a commit subject before it is truncated.
const MAX_SUBJECT_LEN: usize = 120;

/// Arguments for the git_log tool.
#[derive(Debug, Serialize, Deserialize)]
pub struct GitLogArgs {
    /// Relative path to the file within the repository.
    pub path: String,
    /// Maximum number of commits to return (defaults to
    /// [`DEFAULT_MAX_COUNT`], capped at [`MAX_COUNT_CEILING`]).
    #[serde(default)]
    pub max_count: Option<usize>,
}

/// Error type for the git_log tool.
#[derive(Debug, thiserror::Error)]
#[error("GitLog error: {0}")]
pub struct GitLogError(pub String);

/// Rig-core tool that shows a file's recent commit history.
#[derive(Serialize, Deserialize)]
pub struct GitLogTool {
    repo_root: PathBuf,
}

impl GitLogTool {
    /// Create a new GitLogTool anchored at the given repo root.
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

impl Tool for GitLogTool {
    const NAME: &'static str = "git_log";
    type Error = GitLogError;
    type Args = GitLogArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "git_log".to_string(),
            description: format!(
                "Show the recent commit history of a file in the repository \
                 (followed across renames). Each entry is `<short-sha>  <date>  \
                 <subject>  — <author>`, newest first, up to {DEFAULT_MAX_COUNT} \
                 commits by default (max {MAX_COUNT_CEILING}). Use this to judge \
                 a change against its past: whether it reverts or re-introduces \
                 something a prior commit deliberately changed or fixed, or \
                 contradicts the intent recorded in earlier commit messages. \
                 The diff shows *what* changed; this shows *why* the code is the \
                 way it is."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the repository (e.g. `src/main.rs`)."
                    },
                    "max_count": {
                        "type": "integer",
                        "description": format!(
                            "Maximum commits to return. Defaults to {DEFAULT_MAX_COUNT}, capped at {MAX_COUNT_CEILING}."
                        ),
                        "minimum": 1
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Err(msg) = crate::tools::budget::try_consume("git_log") {
            return Err(GitLogError(msg));
        }
        let start = crate::tools::start_tool_call();

        let memo_key = serde_json::json!({
            "repo": self.repo_root.display().to_string(),
            "args": &args,
        });
        if let Some(hit) = crate::tools::memo::lookup("git_log", &memo_key) {
            crate::tools::finish_tool_call(start, "git_log", &args.path, "cached");
            return Ok(hit);
        }

        let max_count = args
            .max_count
            .unwrap_or(DEFAULT_MAX_COUNT)
            .clamp(1, MAX_COUNT_CEILING);

        let commits = git_log_for_file(&self.repo_root, &args.path, max_count)
            .await
            .map_err(GitLogError)?;

        let result_summary = if commits.is_empty() {
            "no history".to_string()
        } else {
            format!(
                "{} commit{}",
                commits.len(),
                if commits.len() == 1 { "" } else { "s" }
            )
        };
        crate::tools::finish_tool_call(start, "git_log", &args.path, result_summary);

        let body = if commits.is_empty() {
            format!(
                "No commit history for `{}` (new/untracked file, or outside the repo's history).",
                args.path
            )
        } else {
            commits.join("\n")
        };
        crate::tools::memo::store("git_log", &memo_key, body.clone());
        Ok(body)
    }
}

/// Returns true if `relative_path` would resolve outside the repo root
/// by purely logical means (absolute path, or `..` segments that pop
/// past the start), without touching the filesystem.
fn escapes_repo_root(relative_path: &str) -> bool {
    use std::path::Component;
    let mut depth: i32 = 0;
    for component in Path::new(relative_path).components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return true,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::Normal(_) => depth += 1,
        }
    }
    false
}

/// Run `git log --follow` for a single file and return formatted,
/// newest-first one-line summaries.
///
/// The path is passed after `--` and never interpolated into a shell,
/// so a hostile filename cannot inject arguments. Paths that escape the
/// repo root are rejected before `git` is invoked.
pub async fn git_log_for_file(
    repo_root: &Path,
    relative_path: &str,
    max_count: usize,
) -> Result<Vec<String>, String> {
    if escapes_repo_root(relative_path) {
        return Err(format!("path traversal blocked: {relative_path}"));
    }

    let max_count_arg = format!("--max-count={max_count}");
    // Tab-separated fields keep parsing unambiguous even when the
    // subject contains spaces or em-dashes.
    let output = tokio::process::Command::new("git")
        .args([
            "log",
            "--no-color",
            "--follow",
            &max_count_arg,
            "--date=short",
            "--pretty=format:%h%x09%ad%x09%an%x09%s",
            "--",
            relative_path,
        ])
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| format!("failed to run git log: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git log failed (exit {}): {}",
            output.status,
            stderr.trim()
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let commits = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(format_commit_line)
        .collect();
    Ok(commits)
}

/// Format one `%h\t%ad\t%an\t%s` record into a compact display line.
///
/// Falls back to the raw line if the record does not have the expected
/// four tab-separated fields (defensive against an unexpected git output
/// shape — better a raw line than a dropped commit).
fn format_commit_line(raw: &str) -> String {
    let parts: Vec<&str> = raw.splitn(4, '\t').collect();
    if parts.len() < 4 {
        return raw.to_string();
    }
    let (sha, date, author, subject) = (parts[0], parts[1], parts[2], parts[3]);
    let subject = truncate_subject(subject);
    format!("{sha}  {date}  {subject}  — {author}")
}

/// Truncate a commit subject to [`MAX_SUBJECT_LEN`] characters, appending
/// an ellipsis when shortened. Counts characters, not bytes, so a
/// multi-byte subject is never split mid-codepoint.
fn truncate_subject(subject: &str) -> String {
    if subject.chars().count() <= MAX_SUBJECT_LEN {
        return subject.to_string();
    }
    let mut s: String = subject.chars().take(MAX_SUBJECT_LEN).collect();
    s.push('…');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway git repo with two commits touching `file.rs`.
    async fn repo_with_history() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Tester"]);
        std::fs::write(path.join("file.rs"), "fn a() {}\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "Add a()"]);
        std::fs::write(path.join("file.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "Add b()"]);
        dir
    }

    #[tokio::test]
    async fn lists_history_newest_first() {
        let dir = repo_with_history().await;
        let commits = git_log_for_file(dir.path(), "file.rs", 10).await.unwrap();
        assert_eq!(commits.len(), 2);
        // Newest first.
        assert!(commits[0].contains("Add b()"));
        assert!(commits[1].contains("Add a()"));
        // Date + author rendered.
        assert!(commits[0].contains("— Tester"));
    }

    #[tokio::test]
    async fn respects_max_count() {
        let dir = repo_with_history().await;
        let commits = git_log_for_file(dir.path(), "file.rs", 1).await.unwrap();
        assert_eq!(commits.len(), 1);
        assert!(commits[0].contains("Add b()"));
    }

    #[tokio::test]
    async fn empty_for_untracked_file() {
        let dir = repo_with_history().await;
        let commits = git_log_for_file(dir.path(), "nope.rs", 10).await.unwrap();
        assert!(commits.is_empty());
    }

    #[tokio::test]
    async fn blocks_path_traversal() {
        let dir = repo_with_history().await;
        let err = git_log_for_file(dir.path(), "../../etc/passwd", 10)
            .await
            .unwrap_err();
        assert!(err.contains("traversal"), "got: {err}");
    }

    #[tokio::test]
    async fn call_renders_no_history_message() {
        let dir = repo_with_history().await;
        let tool = GitLogTool::new(dir.path().to_path_buf());
        let out = Tool::call(
            &tool,
            GitLogArgs {
                path: "nope.rs".to_string(),
                max_count: None,
            },
        )
        .await
        .unwrap();
        assert!(out.contains("No commit history"));
    }

    #[tokio::test]
    async fn call_caps_max_count_at_ceiling() {
        let dir = repo_with_history().await;
        let tool = GitLogTool::new(dir.path().to_path_buf());
        // A huge request still succeeds; the clamp is internal.
        let out = Tool::call(
            &tool,
            GitLogArgs {
                path: "file.rs".to_string(),
                max_count: Some(10_000),
            },
        )
        .await
        .unwrap();
        assert!(out.contains("Add b()"));
        assert!(out.contains("Add a()"));
    }

    #[test]
    fn format_commit_line_parses_fields() {
        let line = format_commit_line("abc1234\t2026-06-05\tAlice\tFix the thing");
        assert_eq!(line, "abc1234  2026-06-05  Fix the thing  — Alice");
    }

    #[test]
    fn format_commit_line_falls_back_on_bad_shape() {
        let line = format_commit_line("not-tab-separated");
        assert_eq!(line, "not-tab-separated");
    }

    #[test]
    fn truncate_subject_caps_length() {
        let long = "x".repeat(MAX_SUBJECT_LEN + 50);
        let out = truncate_subject(&long);
        assert!(out.chars().count() <= MAX_SUBJECT_LEN + 1);
        assert!(out.ends_with('…'));
    }

    #[tokio::test]
    async fn definition_has_correct_name() {
        let tool = GitLogTool::new(PathBuf::from("/tmp"));
        let def = Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "git_log");
        assert!(def.description.contains("revert"));
    }
}
