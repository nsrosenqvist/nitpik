//! Bitbucket Code Insights Annotations API renderer.
//!
//! Creates reports and annotations via the Bitbucket API using reqwest.

use crate::models::finding::{Finding, Severity, Summary};
use crate::output::OutputRenderer;
use thiserror::Error;

/// Errors from Bitbucket API calls.
#[derive(Error, Debug)]
pub enum BitbucketError {
    #[error("missing environment variable: {0}")]
    MissingEnvVar(String),

    #[error("API request failed: {0}")]
    ApiError(String),
}

/// Bitbucket Code Insights renderer.
///
/// For non-async rendering, this outputs JSON that can be posted
/// to the Bitbucket API. For actual API calls, use `post_to_bitbucket`.
pub struct BitbucketRenderer;

impl OutputRenderer for BitbucketRenderer {
    fn render(&self, findings: &[Finding]) -> String {
        let annotations: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                let severity = match f.severity {
                    Severity::Error => "HIGH",
                    Severity::Warning => "MEDIUM",
                    Severity::Info => "LOW",
                };
                let mut message = f.message.clone();
                if let Some(ref suggestion) = f.suggestion {
                    message.push_str(&format!("\n\nSuggestion: {suggestion}"));
                }
                serde_json::json!({
                    "path": f.file,
                    "line": f.line,
                    "message": message,
                    "severity": severity,
                    "type": "CODE_SMELL",
                    "summary": f.title,
                })
            })
            .collect();

        serde_json::to_string_pretty(&serde_json::json!({
            "annotations": annotations
        }))
        .unwrap_or_else(|_| "{}".to_string())
    }
}

/// Post findings to the Bitbucket Code Insights API.
///
/// Requires these env vars: `BITBUCKET_WORKSPACE`, `BITBUCKET_REPO_SLUG`,
/// `BITBUCKET_COMMIT`, `BITBUCKET_TOKEN`.
///
/// `fail_on` controls the report result: if any finding meets or exceeds
/// the threshold, the report is marked `FAILED`. When `None`, only errors
/// cause a `FAILED` result.
pub async fn post_to_bitbucket(
    findings: &[Finding],
    fail_on: Option<Severity>,
) -> Result<(), BitbucketError> {
    let workspace =
        std::env::var("BITBUCKET_WORKSPACE").map_err(|_| BitbucketError::MissingEnvVar("BITBUCKET_WORKSPACE".into()))?;
    let repo_slug =
        std::env::var("BITBUCKET_REPO_SLUG").map_err(|_| BitbucketError::MissingEnvVar("BITBUCKET_REPO_SLUG".into()))?;
    let commit =
        std::env::var("BITBUCKET_COMMIT").map_err(|_| BitbucketError::MissingEnvVar("BITBUCKET_COMMIT".into()))?;
    let token =
        std::env::var("BITBUCKET_TOKEN").map_err(|_| BitbucketError::MissingEnvVar("BITBUCKET_TOKEN".into()))?;

    let client = reqwest::Client::new();
    let base_url = format!(
        "https://api.bitbucket.org/2.0/repositories/{workspace}/{repo_slug}/commit/{commit}"
    );
    let report_id = format!("{}-review", crate::constants::APP_NAME);

    // Step 1: Create/update the report
    let summary = Summary::from_findings(findings);
    let threshold = fail_on.unwrap_or(Severity::Error);
    let should_fail = findings.iter().any(|f| f.severity >= threshold);
    let result = if should_fail { "FAILED" } else { "PASSED" };

    let report_body = serde_json::json!({
        "title": format!("{} Code Review", crate::constants::APP_NAME),
        "details": format!(
            "{} findings: {} errors, {} warnings, {} info. {}",
            summary.total, summary.errors, summary.warnings, summary.info,
            crate::constants::AI_DISCLOSURE,
        ),
        "report_type": "BUG",
        "result": result,
    });

    let report_response = client
        .put(format!("{base_url}/reports/{report_id}"))
        .bearer_auth(&token)
        .json(&report_body)
        .send()
        .await
        .map_err(|e| BitbucketError::ApiError(e.to_string()))?;

    if !report_response.status().is_success() {
        let status = report_response.status();
        let body = report_response
            .text()
            .await
            .unwrap_or_else(|_| "<no body>".to_string());
        return Err(BitbucketError::ApiError(format!(
            "report creation failed with HTTP {status}: {body}"
        )));
    }

    // Step 2: Post annotations (in batches of 100)
    let annotations: Vec<serde_json::Value> = findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let severity = match f.severity {
                Severity::Error => "HIGH",
                Severity::Warning => "MEDIUM",
                Severity::Info => "LOW",
            };
            let mut message = f.message.clone();
            if let Some(ref suggestion) = f.suggestion {
                message.push_str(&format!("\n\nSuggestion: {suggestion}"));
            }
            serde_json::json!({
                "external_id": format!("{}-{i}", crate::constants::APP_NAME),
                "path": f.file,
                "line": f.line,
                "summary": f.title,
                "details": message,
                "annotation_type": "CODE_SMELL",
                "severity": severity,
            })
        })
        .collect();

    for chunk in annotations.chunks(100) {
        let ann_response = client
            .post(format!("{base_url}/reports/{report_id}/annotations"))
            .bearer_auth(&token)
            .json(&chunk.to_vec())
            .send()
            .await
            .map_err(|e| BitbucketError::ApiError(e.to_string()))?;

        if !ann_response.status().is_success() {
            let status = ann_response.status();
            let body = ann_response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(BitbucketError::ApiError(format!(
                "annotation post failed with HTTP {status}: {body}"
            )));
        }
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
        let findings = sample_findings();
        let output = BitbucketRenderer.render(&findings);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let annotations = parsed["annotations"].as_array().unwrap();
        assert_eq!(annotations.len(), 2);
    }

    #[test]
    fn render_maps_severity_correctly() {
        let findings = vec![
            Finding {
                file: "a.rs".to_string(),
                line: 1,
                end_line: None,
                severity: Severity::Error,
                title: "E".to_string(),
                message: "error".to_string(),
                suggestion: None,
                agent: "t".to_string(),
            },
            Finding {
                file: "b.rs".to_string(),
                line: 2,
                end_line: None,
                severity: Severity::Warning,
                title: "W".to_string(),
                message: "warning".to_string(),
                suggestion: None,
                agent: "t".to_string(),
            },
            Finding {
                file: "c.rs".to_string(),
                line: 3,
                end_line: None,
                severity: Severity::Info,
                title: "I".to_string(),
                message: "info".to_string(),
                suggestion: None,
                agent: "t".to_string(),
            },
        ];
        let output = BitbucketRenderer.render(&findings);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let annotations = parsed["annotations"].as_array().unwrap();
        assert_eq!(annotations[0]["severity"], "HIGH");
        assert_eq!(annotations[1]["severity"], "MEDIUM");
        assert_eq!(annotations[2]["severity"], "LOW");
    }

    #[test]
    fn render_includes_suggestion_in_message() {
        let findings = sample_findings();
        let output = BitbucketRenderer.render(&findings);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let annotations = parsed["annotations"].as_array().unwrap();
        // First finding has a suggestion
        let msg = annotations[0]["message"].as_str().unwrap();
        assert!(msg.contains("Suggestion: Fix the bug"));
        // Second finding has no suggestion
        let msg2 = annotations[1]["message"].as_str().unwrap();
        assert!(!msg2.contains("Suggestion"));
    }

    #[test]
    fn render_empty_findings() {
        let output = BitbucketRenderer.render(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let annotations = parsed["annotations"].as_array().unwrap();
        assert!(annotations.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn post_missing_env_vars_cascade() {
        // Test all four env var checks in sequence within a single test
        // to avoid race conditions from parallel test execution.

        // Guard to clean up env vars even on panic
        struct BitbucketEnvGuard;
        impl Drop for BitbucketEnvGuard {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("BITBUCKET_WORKSPACE");
                    std::env::remove_var("BITBUCKET_REPO_SLUG");
                    std::env::remove_var("BITBUCKET_COMMIT");
                    std::env::remove_var("BITBUCKET_TOKEN");
                }
            }
        }
        let _guard = BitbucketEnvGuard;

        unsafe {
            std::env::remove_var("BITBUCKET_WORKSPACE");
            std::env::remove_var("BITBUCKET_REPO_SLUG");
            std::env::remove_var("BITBUCKET_COMMIT");
            std::env::remove_var("BITBUCKET_TOKEN");
        }

        // Missing BITBUCKET_WORKSPACE
        let result = post_to_bitbucket(&sample_findings(), None).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("BITBUCKET_WORKSPACE"),
            "expected BITBUCKET_WORKSPACE error"
        );

        // Missing BITBUCKET_REPO_SLUG
        unsafe { std::env::set_var("BITBUCKET_WORKSPACE", "test-ws"); }
        let result = post_to_bitbucket(&sample_findings(), None).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("BITBUCKET_REPO_SLUG"),
            "expected BITBUCKET_REPO_SLUG error"
        );

        // Missing BITBUCKET_COMMIT
        unsafe { std::env::set_var("BITBUCKET_REPO_SLUG", "test-repo"); }
        let result = post_to_bitbucket(&sample_findings(), None).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("BITBUCKET_COMMIT"),
            "expected BITBUCKET_COMMIT error"
        );

        // Missing BITBUCKET_TOKEN
        unsafe { std::env::set_var("BITBUCKET_COMMIT", "abc123"); }
        let result = post_to_bitbucket(&sample_findings(), None).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("BITBUCKET_TOKEN"),
            "expected BITBUCKET_TOKEN error"
        );

        // _guard cleanup happens automatically via Drop
    }
}
