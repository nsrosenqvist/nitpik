//! SearchTextTool — search for text patterns in the repository.
//!
//! Implements rig-core's `Tool` trait for native agentic tool calling.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use regex::Regex;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Maximum number of search results to return.
const MAX_RESULTS: usize = 50;

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

/// A single search result.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub file: String,
    pub line_number: u32,
    pub content: String,
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
            description: "Search for text patterns in the repository. \
                Returns matching lines with file paths and line numbers. \
                Useful for finding function definitions, usages, imports, etc."
                .to_string(),
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
        let start = crate::tools::start_tool_call();
        let results = search_text(&self.repo_root, &args.pattern, args.is_regex)
            .await
            .map_err(SearchTextError)?;

        let result_summary = if results.is_empty() {
            "no matches".to_string()
        } else {
            format!(
                "{} result{}",
                results.len(),
                if results.len() == 1 { "" } else { "s" }
            )
        };
        let args_summary = if args.pattern.len() > 40 {
            format!("\"{}...\"", &args.pattern[..37])
        } else {
            format!("\"{}\"", &args.pattern)
        };
        crate::tools::finish_tool_call(start, "search_text", args_summary, result_summary);

        // Format results as a human-readable string for the LLM
        if results.is_empty() {
            return Ok("No matches found.".to_string());
        }

        let formatted: Vec<String> = results
            .iter()
            .map(|r| format!("{}:{}: {}", r.file, r.line_number, r.content))
            .collect();

        Ok(formatted.join("\n"))
    }
}

/// Search for a text pattern in the repository.
///
/// Uses gitignore-aware file traversal and supports both literal
/// and regex patterns.
pub async fn search_text(
    repo_root: &Path,
    pattern: &str,
    is_regex: bool,
) -> Result<Vec<SearchResult>, String> {
    let regex = if is_regex {
        Regex::new(pattern).map_err(|e| format!("invalid regex: {e}"))?
    } else {
        Regex::new(&regex::escape(pattern)).map_err(|e| format!("regex error: {e}"))?
    };

    let root = repo_root.to_path_buf();
    let regex_clone = regex.clone();

    // Run file traversal in a blocking task since walkdir is synchronous
    let results = tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        let walker = WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .build();

        'outer: for entry in walker.flatten() {
            if entry.file_type().is_none_or(|ft| !ft.is_file()) {
                continue;
            }

            // Skip binary and very large files
            if let Ok(metadata) = entry.metadata() {
                if metadata.len() > 1024 * 1024 {
                    continue;
                }
            }

            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let relative_path = entry
                .path()
                .strip_prefix(&root)
                .unwrap_or(entry.path())
                .display()
                .to_string();

            for (i, line) in content.lines().enumerate() {
                if regex_clone.is_match(line) {
                    results.push(SearchResult {
                        file: relative_path.clone(),
                        line_number: i as u32 + 1,
                        content: line.to_string(),
                    });

                    if results.len() >= MAX_RESULTS {
                        break 'outer;
                    }
                }
            }
        }

        results
    })
    .await
    .map_err(|e| format!("search task failed: {e}"))?;

    Ok(results)
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

        let results = search_text(dir.path(), "hello", false).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].line_number, 1);
        assert_eq!(results[1].line_number, 3);
    }

    #[tokio::test]
    async fn search_regex() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test.txt"),
            "fn main() {}\nfn hello() {}\nlet x = 1;",
        )
        .unwrap();

        let results = search_text(dir.path(), r"fn \w+\(\)", true).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn search_no_results() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test.txt"), "nothing here").unwrap();

        let results = search_text(dir.path(), "nonexistent", false).await.unwrap();
        assert!(results.is_empty());
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
    async fn call_with_results() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("code.rs"), "fn hello() {}\nfn world() {}").unwrap();

        let tool = SearchTextTool::new(dir.path().to_path_buf());
        let result = Tool::call(
            &tool,
            SearchTextArgs {
                pattern: "fn".to_string(),
                is_regex: false,
            },
        )
        .await
        .unwrap();
        assert!(result.contains("fn hello"));
        assert!(result.contains("fn world"));
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
    }
}
