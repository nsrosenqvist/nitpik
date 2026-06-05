//! GitLab merge-request review output — a thin [`crate::output`] adapter over
//! the [`crate::forge`] layer, the GitLab counterpart of
//! [`crate::output::github_pr_review`].
//!
//! The substance (REST calls, inline-discussion anchoring via the MR
//! `diff_refs`, cross-run dedup, request-changes decision, run footer) lives in
//! [`crate::forge`] and [`crate::forge::gitlab`], shared with the other forges.
//! This module only bridges the two output traits to that layer:
//!
//! - [`GitlabMrReviewFormatter`] renders a neutral JSON preview of the review
//!   (summary note + inline discussions) for `--format gitlab-mr-review`
//!   inspection / piping.
//! - [`GitlabMrReviewPublisher`] posts the review through the shared
//!   [`forge::publish_review`] path, deduping against findings earlier runs
//!   already posted.
//!
//! Unlike the older direct-post `Gitlab` Code Quality artifact (which emits a
//! CodeClimate report), this mode posts a real inline MR review and is
//! dedup-aware and request-changes-capable, exactly like `github-pr-review`.
//!
//! See [`crate::forge::gitlab::GitlabForge::from_env`] for the required GitLab
//! CI environment variables (`CI_API_V4_URL`/`CI_SERVER_URL`, `CI_PROJECT_ID`,
//! `CI_MERGE_REQUEST_IID`, and `GITLAB_TOKEN` or `CI_JOB_TOKEN`).

use std::collections::HashMap;

use crate::env::Env;
use crate::forge::{self, ReviewEvent, gitlab::GitlabForge};
use crate::models::finding::Finding;
use crate::output::{OutputFormatter, OutputPublisher};

/// GitLab MR review formatter — renders a neutral JSON preview.
pub struct GitlabMrReviewFormatter;

impl OutputFormatter for GitlabMrReviewFormatter {
    fn format(&self, findings: &[Finding]) -> String {
        // Pure preview: the run-link footer, cross-run dedup, and the
        // request-changes decision are applied by the publisher. The rendered
        // payload comments on every finding as a plain COMMENT. GitLab posts a
        // summary note plus one positioned discussion per comment, so the
        // preview mirrors that neutral shape rather than a single API payload.
        let draft = forge::build_review_draft(findings, findings, "", ReviewEvent::Comment);
        let discussions: Vec<serde_json::Value> = draft
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
            "discussions": discussions,
        });
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    }
}

/// GitLab MR review publisher.
pub struct GitlabMrReviewPublisher<'a> {
    env: &'a Env,
    event: ReviewEvent,
    force: bool,
    corroboration: HashMap<String, u32>,
}

impl<'a> GitlabMrReviewPublisher<'a> {
    /// `event` is the review action to post (typically derived from findings
    /// via [`forge::review_event_for`]). `force` posts a review even when a
    /// prior run already covered the MR (bypasses the quiet-on-re-run gate),
    /// for an explicit re-trigger.
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

impl OutputPublisher for GitlabMrReviewPublisher<'_> {
    async fn publish(
        &self,
        findings: &[Finding],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let forge = GitlabForge::from_env(self.env)?;
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
    fn formatter_renders_gitlab_review_preview() {
        let out = GitlabMrReviewFormatter.format(&sample_findings());
        let p: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(p["summary"].as_str().unwrap().contains("1 finding"));
        assert_eq!(p["discussions"][0]["path"], "src/main.rs");
        assert_eq!(p["discussions"][0]["line"], 10);
        assert!(
            p["discussions"][0]["body"]
                .as_str()
                .unwrap()
                .contains("Bug")
        );
    }

    #[tokio::test]
    async fn publish_errors_without_gitlab_env() {
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let err = GitlabMrReviewPublisher::new(&env, ReviewEvent::Comment, false)
            .publish(&sample_findings())
            .await
            .unwrap_err();
        // from_env requires the MR context; surfaces a missing-env error.
        assert!(
            err.to_string().contains("CI_API_V4_URL")
                || err.to_string().contains("CI_PROJECT_ID")
                || err.to_string().contains("CI_MERGE_REQUEST_IID"),
            "got: {err}"
        );
    }
}
