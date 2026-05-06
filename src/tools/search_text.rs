//! SearchTextTool — search for text patterns in the repository.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::format::{LINE_NUMBER_WIDTH, format_byte_size};

/// Maximum number of distinct match locations returned in a single call.
///
/// Lowered from 50 to keep the context cost of `search_text` predictable
/// once we surround every match with a window of context lines.
const MAX_RESULTS: usize = 25;

/// Number of source lines emitted before and after each matching line.
///
/// Two lines on each side gives the LLM enough surrounding code to
/// disambiguate a match without ballooning the response.
const CONTEXT_LINES: usize = 2;

/// Soft cap on the rendered output, in bytes.
const OUTPUT_BYTE_BUDGET: usize = 60 * 1024;

/// Maximum wall-clock time allowed for a search operation.
const SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Skip files larger than this on disk — likely binary or generated.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Arguments for the search_text tool.
#[derive(Debug, Deserialize)]
pub struct SearchTextArgs {
    /// The text pattern to search for.
    pub pattern: String,
    /// Whether to interpret the pattern as a regex (default: false).
    #[serde(default)]
    pub is_regex: bool,
}

/// Error type for the search_text tool.
#[derive(Debug, thiserror::Error)]
#[error("SearchText error: {0}")]
pub struct SearchTextError(pub String);

/// A single search result with surrounding context lines.
#[derive(Debug, Serialize, Clone)]
pub struct SearchResult {
    /// Repo-relative path to the matching file.
    pub file: String,
    /// 1-based line number of the matched line.
    pub line_number: u32,
    /// Content of the matched line (without the leading line-number column).
    pub content: String,
    /// Lines preceding the match, oldest first. Up to [`CONTEXT_LINES`].
    pub before: Vec<String>,
    /// Lines following the match, in source order. Up to [`CONTEXT_LINES`].
    pub after: Vec<String>,
}

/// Aggregate result of a [`search_text`] invocation.
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    /// Matches returned, capped at [`MAX_RESULTS`].
    pub results: Vec<SearchResult>,
    /// Total number of matches encountered before the cap was applied.
    pub total_matches: usize,
    /// True when the [`MAX_RESULTS`] cap was hit during traversal.
    pub truncated_to_max_results: bool,
}

/// Rig-core tool that searches for text patterns in the repository.
#[derive(Serialize, Deserialize)]
pub struct SearchTextTool {
    repo_root: PathBuf,
}

impl SearchTextTool {
    /// Create a new SearchTextTool anchored at the given repo root.
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root }
    }
}

impl Tool for SearchTextTool {
    const NAME: &'static str = "search_text";
    type Error = SearchTextError;
    type Args = SearchTextArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: "search_text".to_string(),
            description: format!(
                "Search for text patterns in the repository. Returns up to \
                 {MAX_RESULTS} matches with {CONTEXT_LINES} lines of context \
                 above and below each hit. Each match is rendered as a \
                 `path:line` header followed by line-numbered source. \
                 Honors `.gitignore` and skips files larger than {}. When \
                 more than {MAX_RESULTS} matches exist the response is \
                 truncated with a footer telling you how many were omitted; \
                 narrow the pattern to see the rest.",
                format_byte_size(MAX_FILE_BYTES as usize)
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "The text pattern to search for"
                    },
                    "is_regex": {
                        "type": "boolean",
                        "description": "Whether to interpret the pattern as a regular expression (default: false)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        if let Err(msg) = crate::tools::budget::try_consume("search_text") {
            return Err(SearchTextError(msg));
        }
        let start = crate::tools::start_tool_call();
        let outcome = search_text(&self.repo_root, &args.pattern, args.is_regex)
            .await
            .map_err(SearchTextError)?;

        let body = render_results(&outcome);

        let result_summary = if outcome.results.is_empty() {
            "no matches".to_string()
        } else {
            let suffix = if outcome.truncated_to_max_results {
                format!(" (+{} more)", outcome.total_matches - outcome.results.len())
            } else {
                String::new()
            };
            format!(
                "{} match{}{suffix}",
                outcome.results.len(),
                if outcome.results.len() == 1 { "" } else { "es" }
            )
        };

        let args_summary = if args.pattern.len() > 40 {
            format!("\"{}...\"", &args.pattern[..37])
        } else {
            format!("\"{}\"", &args.pattern)
        };
        crate::tools::finish_tool_call(start, "search_text", args_summary, result_summary);

        Ok(body)
    }
}

/// Search for a text pattern in the repository.
///
/// Uses gitignore-aware file traversal and supports both literal
/// and regex patterns. Each result carries [`CONTEXT_LINES`] lines of
/// surrounding source so the caller can ground the hit without an
/// extra `read_file` round-trip.
pub async fn search_text(
    repo_root: &Path,
    pattern: &str,
    is_regex: bool,
) -> Result<SearchOutcome, String> {
    use regex::RegexBuilder;

    let regex = if is_regex {
        RegexBuilder::new(pattern)
            .size_limit(10 * 1024 * 1024)
            .build()
            .map_err(|e| format!("invalid regex: {e}"))?
    } else {
        RegexBuilder::new(&regex::escape(pattern))
            .size_limit(10 * 1024 * 1024)
            .build()
            .map_err(|e| format!("regex error: {e}"))?
    };

    let root = repo_root.to_path_buf();
    let regex_clone = regex.clone();

    let outcome = tokio::time::timeout(
        SEARCH_TIMEOUT,
        tokio::task::spawn_blocking(move || -> SearchOutcome {
            let mut results: Vec<SearchResult> = Vec::new();
            let mut total: usize = 0;
            let mut hit_cap = false;
            let walker = WalkBuilder::new(&root)
                .hidden(true)
                .git_ignore(true)
                .build();

            for entry in walker.flatten() {
                if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                    continue;
                }

                if let Ok(metadata) = entry.metadata() {
                    if metadata.len() > MAX_FILE_BYTES {
                        continue;
                    }
                }

                let Ok(content) = std::fs::read_to_string(entry.path()) else {
                    continue;
                };

                let relative_path = entry
                    .path()
                    .strip_prefix(&root)
                    .unwrap_or(entry.path())
                    .display()
                    .to_string();

                let lines: Vec<&str> = content.lines().collect();

                for (i, line) in lines.iter().enumerate() {
                    if regex_clone.is_match(line) {
                        total += 1;
                        if results.len() >= MAX_RESULTS {
                            hit_cap = true;
                            continue;
                        }

                        let start = i.saturating_sub(CONTEXT_LINES);
                        let end = (i + CONTEXT_LINES + 1).min(lines.len());
                        let before: Vec<String> =
                            lines[start..i].iter().map(|s| (*s).to_string()).collect();
                        let after: Vec<String> =
                            lines[i + 1..end].iter().map(|s| (*s).to_string()).collect();

                        results.push(SearchResult {
                            file: relative_path.clone(),
                            line_number: i as u32 + 1,
                            content: (*line).to_string(),
                            before,
                            after,
                        });
                    }
                }
            }

            SearchOutcome {
                results,
                total_matches: total,
                truncated_to_max_results: hit_cap,
            }
        }),
    )
    .await
    .map_err(|_| format!("search timed out after {}s", SEARCH_TIMEOUT.as_secs()))?
    .map_err(|e| format!("search task failed: {e}"))?;

    Ok(outcome)
}

/// Render a [`SearchOutcome`] into the line-numbered, blocked text the
/// LLM consumes.
fn render_results(outcome: &SearchOutcome) -> String {
    if outcome.results.is_empty() {
        return "No matches found.".to_string();
    }

    let mut out = String::new();
    let mut byte_truncated_at: Option<usize> = None;

    for (idx, r) in outcome.results.iter().enumerate() {
        let mut block = String::new();
        block.push_str(&format!("{}:{}\n", r.file, r.line_number));

        let first_ctx_line = (r.line_number as usize).saturating_sub(r.before.len());
        let mut n = first_ctx_line.max(1);
        for line in &r.before {
            block.push_str(&format!("{n:>LINE_NUMBER_WIDTH$} | {line}\n"));
            n += 1;
        }
        // The match line itself, marked with '>' before the pipe so the
        // LLM can distinguish it from context.
        block.push_str(&format!(
            "{:>LINE_NUMBER_WIDTH$} > {}\n",
            r.line_number, r.content
        ));
        n = r.line_number as usize + 1;
        for line in &r.after {
            block.push_str(&format!("{n:>LINE_NUMBER_WIDTH$} | {line}\n"));
            n += 1;
        }

        // Apply byte budget. Always emit at least one block.
        if !out.is_empty() && out.len() + 1 + block.len() > OUTPUT_BYTE_BUDGET {
            byte_truncated_at = Some(idx);
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&block);
    }

    if out.ends_with('\n') {
        out.pop();
    }

    let omitted_byte = byte_truncated_at
        .map(|i| outcome.results.len() - i)
        .unwrap_or(0);
    let omitted_cap = if outcome.truncated_to_max_results {
        outcome.total_matches.saturating_sub(outcome.results.len())
    } else {
        0
    };
    let total_omitted = omitted_byte + omitted_cap;

    if total_omitted > 0 {
        let mut reasons: Vec<String> = Vec::new();
        if omitted_byte > 0 {
            reasons.push(format!("{omitted_byte} dropped to fit byte budget"));
        }
        if omitted_cap > 0 {
            reasons.push(format!("{omitted_cap} beyond {MAX_RESULTS}-match cap"));
        }
        out.push_str(&format!(
            "\n\n... {total_omitted} more match{} omitted ({}); narrow the pattern to see them",
            if total_omitted == 1 { "" } else { "es" },
            reasons.join(", "),
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_literal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "hello world\nfoo bar\nhello again",
        )
        .unwrap();

        let outcome = search_text(dir.path(), "hello", false).await.unwrap();
        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.results[0].line_number, 1);
        assert_eq!(outcome.results[1].line_number, 3);
        assert!(!outcome.truncated_to_max_results);
        assert_eq!(outcome.total_matches, 2);
    }

    #[tokio::test]
    async fn search_includes_context_lines() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "a\nb\nc\nMATCH\ne\nf\ng").unwrap();

        let outcome = search_text(dir.path(), "MATCH", false).await.unwrap();
        assert_eq!(outcome.results.len(), 1);
        let r = &outcome.results[0];
        assert_eq!(r.before, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(r.after, vec!["e".to_string(), "f".to_string()]);
    }

    #[tokio::test]
    async fn search_context_clamps_at_file_edges() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "MATCH\nb").unwrap();

        let outcome = search_text(dir.path(), "MATCH", false).await.unwrap();
        let r = &outcome.results[0];
        assert!(r.before.is_empty());
        assert_eq!(r.after, vec!["b".to_string()]);
    }

    #[tokio::test]
    async fn search_regex() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "fn main() {}\nfn hello() {}\nlet x = 1;",
        )
        .unwrap();

        let outcome = search_text(dir.path(), r"fn \w+\(\)", true).await.unwrap();
        assert_eq!(outcome.results.len(), 2);
    }

    #[tokio::test]
    async fn search_no_results() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "nothing here").unwrap();

        let outcome = search_text(dir.path(), "nonexistent", false).await.unwrap();
        assert!(outcome.results.is_empty());
    }

    #[tokio::test]
    async fn search_invalid_regex() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "content").unwrap();

        let result = search_text(dir.path(), "[invalid", true).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid regex"));
    }

    #[tokio::test]
    async fn search_caps_at_max_results() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (0..30).map(|_| "match\n").collect();
        std::fs::write(dir.path().join("test.txt"), body).unwrap();

        let outcome = search_text(dir.path(), "match", false).await.unwrap();
        assert_eq!(outcome.results.len(), MAX_RESULTS);
        assert!(outcome.truncated_to_max_results);
        assert_eq!(outcome.total_matches, 30);
    }

    #[tokio::test]
    async fn call_renders_blocks_with_line_numbers_and_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("code.rs"),
            "fn outer() {\n    let x = 1;\n    foo();\n    let y = 2;\n}\n",
        )
        .unwrap();

        let tool = SearchTextTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            SearchTextArgs {
                pattern: "foo()".to_string(),
                is_regex: false,
            },
        )
        .await
        .unwrap();

        assert!(result.starts_with("code.rs:3"));
        assert!(result.contains("    3 >     foo();"));
        assert!(result.contains("    1 | fn outer() {"));
        assert!(result.contains("    2 |     let x = 1;"));
        assert!(result.contains("    4 |     let y = 2;"));
        assert!(result.contains("    5 | }"));
    }

    #[tokio::test]
    async fn call_renders_truncation_footer() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (0..30).map(|i| format!("match line {i}\n")).collect();
        std::fs::write(dir.path().join("test.txt"), body).unwrap();

        let tool = SearchTextTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            SearchTextArgs {
                pattern: "match".to_string(),
                is_regex: false,
            },
        )
        .await
        .unwrap();

        assert!(result.contains("more match"));
        assert!(result.contains("omitted"));
        assert!(result.contains(&format!("{MAX_RESULTS}-match cap")));
    }

    #[tokio::test]
    async fn call_no_results() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "nothing").unwrap();

        let tool = SearchTextTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            SearchTextArgs {
                pattern: "nonexistent".to_string(),
                is_regex: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "No matches found.");
    }

    #[tokio::test]
    async fn definition_has_correct_name() {
        let tool = SearchTextTool::new(PathBuf::from("/tmp"));
        let def = Tool::definition(&tool, String::new()).await;
        assert_eq!(def.name, "search_text");
        assert!(!def.description.is_empty());
        assert!(def.description.contains("context"));
    }

    #[test]
    fn render_byte_budget_truncates_at_block_boundary() {
        let huge_line = "x".repeat(2_000);
        let results: Vec<SearchResult> = (0..200)
            .map(|i| SearchResult {
                file: format!("file{i}.rs"),
                line_number: 1,
                content: huge_line.clone(),
                before: Vec::new(),
                after: Vec::new(),
            })
            .collect();
        let outcome = SearchOutcome {
            total_matches: 200,
            truncated_to_max_results: false,
            results,
        };
        let rendered = render_results(&outcome);
        assert!(rendered.len() < OUTPUT_BYTE_BUDGET * 2);
        assert!(rendered.contains("more match"));
        assert!(rendered.contains("byte budget"));
    }
}
