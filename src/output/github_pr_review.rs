//! GitHub PR review output — a thin [`crate::output`] adapter over the
//! [`crate::forge`] layer.
//!
//! The substance (REST calls, anchoring, cross-run dedup, the review-body
//! builder) lives in [`crate::forge`] and [`crate::forge::github`] so it is
//! shared with other forges. This module only bridges the two output
//! traits to that layer:
//!
//! - [`GithubPrReviewFormatter`] renders the GitHub review API payload as
//!   JSON (for `--format github-pr-review` inspection / piping).
//! - [`GithubPrReviewPublisher`] posts the review using the workflow's
//!   `GITHUB_TOKEN`, deduping against findings earlier runs already posted.
//!
//! See [`crate::forge`] for the neutral data model and the required
//! environment variables.

use crate::env::Env;
use crate::forge::{self, github::GithubForge};
use crate::models::finding::Finding;
use crate::output::{OutputFormatter, OutputPublisher};

/// GitHub PR review formatter — renders the API payload as JSON.
pub struct GithubPrReviewFormatter;

impl OutputFormatter for GithubPrReviewFormatter {
    fn format(&self, findings: &[Finding]) -> String {
        // Pure: the run-link footer and cross-run dedup are applied by the
        // publisher; the rendered payload comments on every finding.
        let draft = forge::build_review_draft(findings, findings, "");
        let payload = forge::github::review_payload(&draft);
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    }
}

/// GitHub PR review publisher.
pub struct GithubPrReviewPublisher<'a> {
    env: &'a Env,
}

impl<'a> GithubPrReviewPublisher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self { env }
    }
}

impl OutputPublisher for GithubPrReviewPublisher<'_> {
    async fn publish(
        &self,
        findings: &[Finding],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let forge = GithubForge::from_env(self.env)?;
        forge::publish_review(&forge, findings).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

    fn sample_findings() -> Vec<Finding> {
        vec![Finding {
            file: "src/main.rs".to_string(),
            line: 10,
            end_line: None,
            severity: Severity::Error,
            title: "Bug".to_string(),
            message: "A bug was found".to_string(),
            suggestion: Some("Fix the bug".to_string()),
            agent: "correctness".to_string(),
            evidence: Vec::new(),
        }]
    }

    #[test]
    fn formatter_renders_github_review_payload() {
        let out = GithubPrReviewFormatter.format(&sample_findings());
        let p: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(p["event"], "COMMENT");
        assert_eq!(p["comments"][0]["side"], "RIGHT");
        assert_eq!(p["comments"][0]["line"], 10);
        assert!(p["body"].as_str().unwrap().contains("1 finding"));
    }

    #[tokio::test]
    async fn publish_errors_without_github_env() {
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let err = GithubPrReviewPublisher::new(&env)
            .publish(&sample_findings())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("GITHUB_TOKEN"), "got: {err}");
    }
}
