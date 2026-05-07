//! GlobTool — match files in the repository against a glob pattern.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.
//! Walks the repository tree using `ignore::WalkBuilder` (gitignore-aware)
//! and filters paths through `globset` so the LLM can find files by name
//! pattern without scanning every entry.

use std::path::PathBuf;

use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Maximum number of paths returned per call. Mirrors the cap on
/// `list_directory` to keep responses predictable.
const MAX_RESULTS: usize = 200;

/// Arguments for the glob tool.
#[derive(Debug, Serialize, Deserialize)]
pub struct GlobArgs {
    /// Glob pattern to match relative paths against (e.g.
    /// `**/*.rs`, `src/**/handler_*.ts`).
    pub pattern: String,
}

/// Error type for the glob tool.
#[derive(Debug, thiserror::Error)]
#[error("Glob error: {0}")]
pub struct GlobError(pub String);

/// Rig-core tool that matches files in the repository against a glob.
#[derive(Serialize, Deserialize)]
pub struct GlobTool {
    repo_root: PathBuf,
}

impl GlobTool {
    /// Create a new GlobTool anchored at the given repo root.
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

impl Tool for GlobTool {
    const NAME: &'static str = "glob";
    type Error = GlobError;
    type Args = GlobArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "glob".to_string(),
            description: format!(
                "Match repository files against a glob pattern. \
                 Returns up to {MAX_RESULTS} relative paths, sorted \
                 lexicographically. Respects `.gitignore` and skips \
                 hidden entries — `node_modules`, `target`, build \
                 outputs, etc. are excluded automatically.\n\n\
                 Patterns use standard glob syntax: `*` matches any \
                 sequence within a path segment, `**` matches any \
                 number of path segments, `?` matches one character, \
                 and `{{a,b}}` matches alternatives. Examples:\n\
                 - `**/*.rs` — every Rust file in the repo\n\
                 - `src/**/test_*.py` — Python tests under `src/`\n\
                 - `tests/fixtures/*.json` — JSON fixtures\n\n\
                 Use this when you know what files you are looking \
                 for by name; use `search_text` when you need to \
                 grep file contents."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern relative to the repo root (e.g. '**/*.rs')."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Err(msg) = crate::tools::budget::try_consume("glob") {
            return Err(GlobError(msg));
        }
        let start = crate::tools::start_tool_call();

        let memo_key = serde_json::json!({
            "repo": self.repo_root.display().to_string(),
            "args": &args,
        });
        if let Some(hit) = crate::tools::memo::lookup("glob", &memo_key) {
            crate::tools::finish_tool_call(start, "glob", &args.pattern, "cached");
            return Ok(hit);
        }

        let result = match glob_paths(&self.repo_root, &args.pattern).await {
            Ok(paths) => paths,
            Err(e) => {
                crate::tools::finish_tool_call(start, "glob", &args.pattern, format!("error: {e}"));
                return Err(GlobError(e));
            }
        };

        let truncated = result.len() > MAX_RESULTS;
        let shown: Vec<String> = result.iter().take(MAX_RESULTS).cloned().collect();
        let total = result.len();
        let summary = if truncated {
            format!("{} match(es) (truncated to {MAX_RESULTS})", total)
        } else {
            format!("{} match(es)", total)
        };
        crate::tools::finish_tool_call(start, "glob", &args.pattern, summary);

        let body = if shown.is_empty() {
            format!("No files match pattern '{}'.", args.pattern)
        } else {
            let mut out = shown.join("\n");
            if truncated {
                out.push_str(&format!(
                    "\n... showing {MAX_RESULTS} of {total} matches; \
                     refine your pattern to see more"
                ));
            }
            out
        };

        crate::tools::memo::store("glob", &memo_key, body.clone());
        Ok(body)
    }
}

/// Walk the repository and return paths matching `pattern`, sorted.
///
/// Public helper exposed for unit tests; the production caller is
/// [`GlobTool::call`].
pub async fn glob_paths(repo_root: &std::path::Path, pattern: &str) -> Result<Vec<String>, String> {
    let matcher: GlobMatcher = Glob::new(pattern)
        .map_err(|e| format!("invalid glob pattern '{pattern}': {e}"))?
        .compile_matcher();

    let root = repo_root.to_path_buf();
    let outcome = tokio::task::spawn_blocking(move || {
        let walker = WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .require_git(false)
            .build();

        let mut paths: Vec<String> = Vec::new();
        for entry in walker.flatten() {
            if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                continue;
            }
            let rel = match entry.path().strip_prefix(&root) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if matcher.is_match(rel) {
                paths.push(rel.display().to_string());
            }
        }
        paths.sort();
        paths
    })
    .await
    .map_err(|e| format!("glob walker join error: {e}"))?;

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::create_dir_all(p.join("tests")).unwrap();
        fs::write(p.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(p.join("src/lib.rs"), "// lib\n").unwrap();
        fs::write(p.join("tests/test_one.rs"), "// t1\n").unwrap();
        fs::write(p.join("README.md"), "# readme\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn glob_matches_rust_files() {
        let dir = make_repo();
        let paths = glob_paths(dir.path(), "**/*.rs").await.unwrap();
        assert_eq!(
            paths,
            vec![
                "src/lib.rs".to_string(),
                "src/main.rs".to_string(),
                "tests/test_one.rs".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn glob_matches_specific_subdir() {
        let dir = make_repo();
        let paths = glob_paths(dir.path(), "src/*.rs").await.unwrap();
        assert_eq!(
            paths,
            vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn glob_returns_empty_when_no_match() {
        let dir = make_repo();
        let paths = glob_paths(dir.path(), "**/*.kt").await.unwrap();
        assert!(paths.is_empty());
    }

    #[tokio::test]
    async fn glob_invalid_pattern_returns_error() {
        let dir = make_repo();
        let err = glob_paths(dir.path(), "[unclosed").await.unwrap_err();
        assert!(err.contains("invalid glob"));
    }

    #[tokio::test]
    async fn glob_respects_gitignore() {
        let dir = make_repo();
        // Create a node_modules dir which is in default ignore but not
        // gitignore — to confirm gitignore handling, write a real
        // .gitignore that excludes ignored.txt.
        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("ignored.txt"), "x").unwrap();
        fs::write(dir.path().join("kept.txt"), "x").unwrap();
        let paths = glob_paths(dir.path(), "*.txt").await.unwrap();
        assert_eq!(paths, vec!["kept.txt".to_string()]);
    }
}
