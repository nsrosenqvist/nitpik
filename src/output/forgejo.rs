//! Forgejo/Gitea Pull Request Review API renderer.
//!
//! Posts findings as inline review comments on a pull request via the
//! Forgejo (or Gitea) API. The stdout output is a JSON payload matching
//! the `CreatePullReviewOptions` schema for debugging/piping.
//!
//! When running inside Woodpecker CI the required environment variables
//! (`CI_FORGE_URL`, `CI_REPO_OWNER`, `CI_REPO_NAME`,
//! `CI_COMMIT_PULL_REQUEST`, `CI_COMMIT_SHA`) are provided automatically.
//! The user only needs to supply `FORGEJO_TOKEN`.

use crate::models::finding::{Finding, Severity};
use crate::output::OutputRenderer;
use thiserror::Error;

/// Errors from Forgejo API calls.
#[derive(Error, Debug)]
pub enum ForgejoError {
    #[error("missing environment variable: {0}")]
    MissingEnvVar(String),

    #[error("invalid pull request index: {0}")]
    InvalidPrIndex(String),

    #[error("API request failed: {0}")]
    ApiError(String),
}

/// Forgejo / Gitea pull request review renderer.
///
/// The synchronous `render()` method outputs a JSON object matching the
/// Forgejo `CreatePullReviewOptions` schema so the payload can be
/// inspected or piped. For actually posting to the API, use
/// [`post_to_forgejo`].
pub struct ForgejoRenderer;

impl OutputRenderer for ForgejoRenderer {
    fn render(&self, findings: &[Finding]) -> String {
        let comments: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                let body = format_comment_body(f);
                serde_json::json!({
                    "path": f.file,
                    "body": body,
                    "new_position": f.line,
                    "old_position": 0,
                })
            })
            .collect();

        let summary = crate::models::finding::Summary::from_findings(findings);
        let body = format!(
            "**{}** found {} {} ({} error{}, {} warning{}, {} info)\n\n_{}_",
            crate::constants::APP_NAME,
            summary.total,
            if summary.total == 1 { "finding" } else { "findings" },
            summary.errors,
            if summary.errors == 1 { "" } else { "s" },
            summary.warnings,
            if summary.warnings == 1 { "" } else { "s" },
            summary.info,
            crate::constants::AI_DISCLOSURE,
        );

        let review = serde_json::json!({
            "event": "COMMENT",
            "body": body,
            "comments": comments,
        });

        serde_json::to_string_pretty(&review).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Format a single finding as a Markdown comment body.
fn format_comment_body(f: &Finding) -> String {
    let severity_emoji = match f.severity {
        Severity::Error => "🔴",
        Severity::Warning => "🟡",
        Severity::Info => "🔵",
    };

    let mut body = format!(
        "{} **{}** ({})\n\n{}",
        severity_emoji, f.title, f.severity, f.message
    );
    if let Some(ref suggestion) = f.suggestion {
        body.push_str(&format!("\n\n**Suggestion:** {suggestion}"));
    }
    body.push_str(&format!("\n\n_— agent: {}_", f.agent));
    body
}

/// Read a required environment variable or return a [`ForgejoError`].
fn require_env(name: &str) -> Result<String, ForgejoError> {
    std::env::var(name).map_err(|_| ForgejoError::MissingEnvVar(name.into()))
}

/// Post findings to the Forgejo/Gitea Pull Request Review API.
///
/// Creates a single review with `event: COMMENT` containing inline
/// comments for every finding.
///
/// # Required environment variables
///
/// | Variable | Source |
/// |---|---|
/// | `CI_FORGE_URL` | Woodpecker built-in (or set manually) |
/// | `CI_REPO_OWNER` | Woodpecker built-in |
/// | `CI_REPO_NAME` | Woodpecker built-in |
/// | `CI_COMMIT_PULL_REQUEST` | Woodpecker built-in |
/// | `CI_COMMIT_SHA` | Woodpecker built-in |
/// | `FORGEJO_TOKEN` | User-provided API token |
pub async fn post_to_forgejo(findings: &[Finding]) -> Result<(), ForgejoError> {
    let forge_url = require_env("CI_FORGE_URL")?;
    let owner = require_env("CI_REPO_OWNER")?;
    let repo = require_env("CI_REPO_NAME")?;
    let pr_index_str = require_env("CI_COMMIT_PULL_REQUEST")?;
    let commit_sha = require_env("CI_COMMIT_SHA")?;
    let token = require_env("FORGEJO_TOKEN")?;

    let pr_index: u64 = pr_index_str
        .parse()
        .map_err(|_| ForgejoError::InvalidPrIndex(pr_index_str.clone()))?;

    // Build the review payload
    let comments: Vec<serde_json::Value> = findings
        .iter()
        .map(|f| {
            let body = format_comment_body(f);
            serde_json::json!({
                "path": f.file,
                "body": body,
                "new_position": f.line,
                "old_position": 0,
            })
        })
        .collect();

    let summary = crate::models::finding::Summary::from_findings(findings);
    let review_body = format!(
        "**{}** found {} {} ({} error{}, {} warning{}, {} info)\n\n_{}_",
        crate::constants::APP_NAME,
        summary.total,
        if summary.total == 1 { "finding" } else { "findings" },
        summary.errors,
        if summary.errors == 1 { "" } else { "s" },
        summary.warnings,
        if summary.warnings == 1 { "" } else { "s" },
        summary.info,
        crate::constants::AI_DISCLOSURE,
    );

    let review_payload = serde_json::json!({
        "event": "COMMENT",
        "body": review_body,
        "commit_id": commit_sha,
        "comments": comments,
    });

    let url = format!(
        "{}/api/v1/repos/{}/{}/pulls/{}/reviews",
        forge_url.trim_end_matches('/'),
        owner,
        repo,
        pr_index,
    );

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("Authorization", format!("token {token}"))
        .header("Content-Type", "application/json")
        .json(&review_payload)
        .send()
        .await
        .map_err(|e| ForgejoError::ApiError(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<no body>".to_string());
        return Err(ForgejoError::ApiError(format!(
            "review creation failed with HTTP {status}: {body}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::{Finding, Severity};
    use serial_test::serial;

    fn sample_findings() -> Vec<Finding> {
        vec![
            Finding {
                file: "src/main.rs".to_string(),
                line: 10,
                end_line: None,
                severity: Severity::Error,
                title: "Bug".to_string(),
                message: "A bug was found".to_string(),
                suggestion: Some("Fix the bug".to_string()),
                agent: "backend".to_string(),
            },
            Finding {
                file: "src/lib.rs".to_string(),
                line: 20,
                end_line: None,
                severity: Severity::Warning,
                title: "Style".to_string(),
                message: "Style issue".to_string(),
                suggestion: None,
                agent: "backend".to_string(),
            },
        ]
    }

    #[test]
    fn render_produces_valid_json() {
        let output = ForgejoRenderer.render(&sample_findings());
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["event"], "COMMENT");
        assert!(parsed["body"].as_str().unwrap().contains("2 findings"));
        let comments = parsed["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2);
    }

    #[test]
    fn render_maps_paths_and_lines() {
        let output = ForgejoRenderer.render(&sample_findings());
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let comments = parsed["comments"].as_array().unwrap();
        assert_eq!(comments[0]["path"], "src/main.rs");
        assert_eq!(comments[0]["new_position"], 10);
        assert_eq!(comments[0]["old_position"], 0);
        assert_eq!(comments[1]["path"], "src/lib.rs");
        assert_eq!(comments[1]["new_position"], 20);
    }

    #[test]
    fn render_includes_severity_emoji() {
        let output = ForgejoRenderer.render(&sample_findings());
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let comments = parsed["comments"].as_array().unwrap();
        let error_body = comments[0]["body"].as_str().unwrap();
        assert!(error_body.contains("🔴"));
        assert!(error_body.contains("Bug"));
        let warning_body = comments[1]["body"].as_str().unwrap();
        assert!(warning_body.contains("🟡"));
    }

    #[test]
    fn render_includes_suggestion_when_present() {
        let output = ForgejoRenderer.render(&sample_findings());
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let comments = parsed["comments"].as_array().unwrap();
        let error_body = comments[0]["body"].as_str().unwrap();
        assert!(error_body.contains("**Suggestion:** Fix the bug"));
        // Second finding has no suggestion
        let warning_body = comments[1]["body"].as_str().unwrap();
        assert!(!warning_body.contains("Suggestion"));
    }

    #[test]
    fn render_includes_agent_attribution() {
        let output = ForgejoRenderer.render(&sample_findings());
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let comments = parsed["comments"].as_array().unwrap();
        let body = comments[0]["body"].as_str().unwrap();
        assert!(body.contains("agent: backend"));
    }

    #[test]
    fn render_empty_findings() {
        let output = ForgejoRenderer.render(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["event"], "COMMENT");
        assert!(parsed["body"].as_str().unwrap().contains("0 findings"));
        let comments = parsed["comments"].as_array().unwrap();
        assert!(comments.is_empty());
    }

    #[test]
    fn render_single_finding_uses_singular() {
        let findings = vec![Finding {
            file: "f.rs".to_string(),
            line: 1,
            end_line: None,
            severity: Severity::Info,
            title: "T".to_string(),
            message: "M".to_string(),
            suggestion: None,
            agent: "a".to_string(),
        }];
        let output = ForgejoRenderer.render(&findings);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed["body"].as_str().unwrap().contains("1 finding"));
        // Must not say "1 findings"
        assert!(!parsed["body"].as_str().unwrap().contains("1 findings"));
    }

    #[test]
    fn format_comment_body_info_severity() {
        let f = Finding {
            file: "x.rs".to_string(),
            line: 5,
            end_line: None,
            severity: Severity::Info,
            title: "Note".to_string(),
            message: "Just a note.".to_string(),
            suggestion: None,
            agent: "architect".to_string(),
        };
        let body = format_comment_body(&f);
        assert!(body.contains("🔵"));
        assert!(body.contains("info"));
        assert!(body.contains("agent: architect"));
    }

    #[tokio::test]
    #[serial]
    async fn post_missing_env_vars_cascade() {
        // Guard to clean up env vars even on panic
        struct ForgejoEnvGuard;
        impl Drop for ForgejoEnvGuard {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("CI_FORGE_URL");
                    std::env::remove_var("CI_REPO_OWNER");
                    std::env::remove_var("CI_REPO_NAME");
                    std::env::remove_var("CI_COMMIT_PULL_REQUEST");
                    std::env::remove_var("CI_COMMIT_SHA");
                    std::env::remove_var("FORGEJO_TOKEN");
                }
            }
        }
        let _guard = ForgejoEnvGuard;

        unsafe {
            std::env::remove_var("CI_FORGE_URL");
            std::env::remove_var("CI_REPO_OWNER");
            std::env::remove_var("CI_REPO_NAME");
            std::env::remove_var("CI_COMMIT_PULL_REQUEST");
            std::env::remove_var("CI_COMMIT_SHA");
            std::env::remove_var("FORGEJO_TOKEN");
        }

        // Missing CI_FORGE_URL
        let result = post_to_forgejo(&sample_findings()).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("CI_FORGE_URL"),
            "expected CI_FORGE_URL error"
        );

        // Missing CI_REPO_OWNER
        unsafe { std::env::set_var("CI_FORGE_URL", "https://codeberg.org"); }
        let result = post_to_forgejo(&sample_findings()).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("CI_REPO_OWNER"),
            "expected CI_REPO_OWNER error"
        );

        // Missing CI_REPO_NAME
        unsafe { std::env::set_var("CI_REPO_OWNER", "test-user"); }
        let result = post_to_forgejo(&sample_findings()).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("CI_REPO_NAME"),
            "expected CI_REPO_NAME error"
        );

        // Missing CI_COMMIT_PULL_REQUEST
        unsafe { std::env::set_var("CI_REPO_NAME", "test-repo"); }
        let result = post_to_forgejo(&sample_findings()).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("CI_COMMIT_PULL_REQUEST"),
            "expected CI_COMMIT_PULL_REQUEST error"
        );

        // Missing CI_COMMIT_SHA
        unsafe { std::env::set_var("CI_COMMIT_PULL_REQUEST", "1"); }
        let result = post_to_forgejo(&sample_findings()).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("CI_COMMIT_SHA"),
            "expected CI_COMMIT_SHA error"
        );

        // Missing FORGEJO_TOKEN
        unsafe { std::env::set_var("CI_COMMIT_SHA", "abc123"); }
        let result = post_to_forgejo(&sample_findings()).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("FORGEJO_TOKEN"),
            "expected FORGEJO_TOKEN error"
        );
    }

    #[tokio::test]
    #[serial]
    async fn post_invalid_pr_index() {
        struct ForgejoEnvGuard;
        impl Drop for ForgejoEnvGuard {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("CI_FORGE_URL");
                    std::env::remove_var("CI_REPO_OWNER");
                    std::env::remove_var("CI_REPO_NAME");
                    std::env::remove_var("CI_COMMIT_PULL_REQUEST");
                    std::env::remove_var("CI_COMMIT_SHA");
                    std::env::remove_var("FORGEJO_TOKEN");
                }
            }
        }
        let _guard = ForgejoEnvGuard;

        unsafe {
            std::env::set_var("CI_FORGE_URL", "https://codeberg.org");
            std::env::set_var("CI_REPO_OWNER", "user");
            std::env::set_var("CI_REPO_NAME", "repo");
            std::env::set_var("CI_COMMIT_PULL_REQUEST", "not-a-number");
            std::env::set_var("CI_COMMIT_SHA", "abc123");
            std::env::set_var("FORGEJO_TOKEN", "tok");
        }

        let result = post_to_forgejo(&sample_findings()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid pull request index"), "got: {err}");
    }
}
