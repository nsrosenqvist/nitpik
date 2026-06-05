//! Bitbucket pull-request review output — a thin [`crate::output`] adapter
//! over the [`crate::forge`] layer, the Bitbucket counterpart of
//! [`crate::output::github_pr_review`] and [`crate::output::gitlab_mr_review`].
//!
//! The substance (REST calls, inline anchoring, cross-run dedup,
//! request-changes decision, run footer) lives in [`crate::forge`] and
//! [`crate::forge::bitbucket`]. This module only bridges the two output traits
//! to that layer:
//!
//! - [`BitbucketPrReviewFormatter`] renders a neutral JSON preview (summary +
//!   inline comments) for `--format bitbucket-pr-review` inspection / piping.
//! - [`BitbucketPrReviewPublisher`] posts the review through the shared
//!   [`forge::publish_review`] path, deduping against findings earlier runs
//!   already posted.
//!
//! Unlike the older `bitbucket` Code Insights annotations (commit-level), this
//! mode posts a real inline PR review and is dedup-aware and
//! request-changes-capable, exactly like `github-pr-review`.
//!
//! See [`crate::forge::bitbucket::BitbucketForge::from_env`] for the required
//! Bitbucket Pipelines environment variables (`BITBUCKET_WORKSPACE`,
//! `BITBUCKET_REPO_SLUG`, `BITBUCKET_PR_ID`, and a `BITBUCKET_TOKEN` access
//! token).

use std::collections::HashMap;

use crate::env::Env;
use crate::forge::{self, ReviewEvent, bitbucket::BitbucketForge};
use crate::models::finding::Finding;
use crate::output::{OutputFormatter, OutputPublisher};

/// Bitbucket PR review formatter — renders a neutral JSON preview.
pub struct BitbucketPrReviewFormatter;

impl OutputFormatter for BitbucketPrReviewFormatter {
    fn format(&self, findings: &[Finding]) -> String {
        // Pure preview: the run-link footer, cross-run dedup, and the
        // request-changes decision are applied by the publisher. The rendered
        // payload comments on every finding as a plain COMMENT. Bitbucket posts
        // a summary comment plus one inline comment per finding, so the preview
        // mirrors that neutral shape rather than a single API payload.
        let draft = forge::build_review_draft(findings, findings, "", ReviewEvent::Comment);
        let comments: Vec<serde_json::Value> = draft
            .comments
            .iter()
            .map(|c| {
                serde_json::json!({
                    "path": c.path,
                    "line": c.line,
                    "body": c.body,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "summary": draft.summary,
            "comments": comments,
        });
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Bitbucket PR review publisher.
pub struct BitbucketPrReviewPublisher<'a> {
    env: &'a Env,
    event: ReviewEvent,
    force: bool,
    corroboration: HashMap<String, u32>,
}

impl<'a> BitbucketPrReviewPublisher<'a> {
    /// `event` is the review action to post (typically derived from findings
    /// via [`forge::review_event_for`]). `force` posts a review even when a
    /// prior run already covered the PR (bypasses the quiet-on-re-run gate).
    pub fn new(env: &'a Env, event: ReviewEvent, force: bool) -> Self {
        Self {
            env,
            event,
            force,
            corroboration: HashMap::new(),
        }
    }

    /// Attach a cross-lens corroboration map (keyed by
    /// [`forge::fingerprint`]) so findings 2+ independent lenses raised are
    /// badged in the posted review.
    pub fn with_corroboration(mut self, corroboration: HashMap<String, u32>) -> Self {
        self.corroboration = corroboration;
        self
    }
}

impl OutputPublisher for BitbucketPrReviewPublisher<'_> {
    async fn publish(
        &self,
        findings: &[Finding],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let forge = BitbucketForge::from_env(self.env)?;
        forge::publish_review_with_corroboration(
            &forge,
            findings,
            self.event,
            self.force,
            &self.corroboration,
        )
        .await?;
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
    fn formatter_renders_bitbucket_review_preview() {
        let out = BitbucketPrReviewFormatter.format(&sample_findings());
        let p: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(p["summary"].as_str().unwrap().contains("1 finding"));
        assert_eq!(p["comments"][0]["path"], "src/main.rs");
        assert_eq!(p["comments"][0]["line"], 10);
        assert!(p["comments"][0]["body"].as_str().unwrap().contains("Bug"));
    }

    #[tokio::test]
    async fn publish_errors_without_bitbucket_env() {
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let err = BitbucketPrReviewPublisher::new(&env, ReviewEvent::Comment, false)
            .publish(&sample_findings())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("BITBUCKET_WORKSPACE")
                || err.to_string().contains("BITBUCKET_REPO_SLUG")
                || err.to_string().contains("BITBUCKET_PR_ID"),
            "got: {err}"
        );
    }
}
