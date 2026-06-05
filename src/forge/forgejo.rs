//! Forgejo / Gitea forge adapter.
//!
//! Reads and posts a PR review via the Forgejo (or Gitea) API v1:
//!
//! - read  — `GET /repos/{owner}/{repo}/pulls/{index}` (PR metadata),
//!   `GET .../pulls/{index}/reviews` + `.../reviews/{id}/comments`
//!   (existing review comments, for cross-run dedup)
//! - write — `POST /repos/{owner}/{repo}/pulls/{index}/reviews`
//!   (`CreatePullReviewOptions`: one review carrying the summary body plus
//!   all inline comments)
//!
//! Inline comments anchor with `new_position` (the new-file line);
//! `old_position` is `0`. A range collapses to its last line. Unlike
//! GitHub/GitLab, Forgejo accepts all inline comments in the single
//! review-creation call, so there is no extra positioning read.
//!
//! **Support status:** implemented against the shared [`Forge`](super::Forge)
//! trait so the abstraction has a third real backend, but **not yet
//! advertised** and not live-tested. The existing one-shot `--format
//! forgejo` path ([`crate::output::forgejo`]) is unchanged; folding it into
//! this adapter is deferred until this path is live-tested.
//!
//! # Required environment variables
//!
//! | Variable | Source |
//! |---|---|
//! | `CI_FORGE_URL` | Woodpecker built-in (or set manually) |
//! | `CI_REPO_OWNER` | Woodpecker built-in |
//! | `CI_REPO_NAME` | Woodpecker built-in |
//! | `CI_COMMIT_PULL_REQUEST` | Woodpecker built-in (PR index) |
//! | `CI_COMMIT_SHA` | Woodpecker built-in (review commit_id) |
//! | `FORGEJO_TOKEN` | User-provided API token |
//!
//! `CI_PIPELINE_URL` (run-link footer) is used when present.

use crate::env::Env;

use super::{ForgeError, InlineComment, PullRequest, ReviewDraft};

/// Forgejo / Gitea forge adapter. Owns everything resolved from the
/// environment.
pub struct ForgejoForge {
    client: reqwest::Client,
    api_base: String,
    owner: String,
    repo: String,
    pr_index: u64,
    commit_sha: String,
    token: String,
    pipeline_url: Option<String>,
}

impl ForgejoForge {
    /// Construct from the environment. Returns an error if the forge URL,
    /// repo, PR index, commit SHA, or token is missing — which is how
    /// [`super::detect`] decides Forgejo is not the active forge.
    pub fn from_env(env: &Env) -> Result<Self, ForgeError> {
        let forge_url = require_env(env, "CI_FORGE_URL")?;
        let owner = require_env(env, "CI_REPO_OWNER")?;
        let repo = require_env(env, "CI_REPO_NAME")?;
        let pr_index_str = require_env(env, "CI_COMMIT_PULL_REQUEST")?;
        let pr_index: u64 = pr_index_str
            .parse()
            .map_err(|_| ForgeError::NoPullRequest)?;
        let commit_sha = require_env(env, "CI_COMMIT_SHA")?;
        let token = require_env(env, "FORGEJO_TOKEN")?;

        let api_base = format!("{}/api/v1", forge_url.trim_end_matches('/'));

        let client = crate::http::build_client()
            .map_err(|e| ForgeError::ApiError(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            api_base,
            owner,
            repo,
            pr_index,
            commit_sha,
            token,
            pipeline_url: env.var("CI_PIPELINE_URL").ok(),
        })
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("Authorization", format!("token {}", self.token))
    }

    fn pulls_url(&self) -> String {
        format!(
            "{}/repos/{}/{}/pulls/{}",
            self.api_base, self.owner, self.repo, self.pr_index
        )
    }
}

#[async_trait::async_trait]
impl super::Forge for ForgejoForge {
    async fn pull_request(&self) -> Result<PullRequest, ForgeError> {
        let resp = self
            .auth(self.client.get(self.pulls_url()))
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ForgeError::ApiError(format!(
                "fetch PR failed with HTTP {}",
                resp.status()
            )));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        Ok(PullRequest {
            number: self.pr_index,
            title: json["title"].as_str().unwrap_or_default().to_string(),
            body: json["body"].as_str().unwrap_or_default().to_string(),
            head_sha: json["head"]["sha"].as_str().unwrap_or_default().to_string(),
            base_ref: json["base"]["ref"].as_str().unwrap_or_default().to_string(),
            author: json["user"]["login"].as_str().map(String::from),
        })
    }

    async fn existing_review_comments(&self) -> Result<Vec<super::ExistingComment>, ForgeError> {
        // Forgejo stores inline comments under each review, so list reviews
        // then gather their comments. Bounded + best-effort: callers fail
        // open, so any error stops collection and yields what we have.
        const MAX_REVIEWS: usize = 100;
        let mut out = Vec::new();

        let reviews_url = format!("{}/reviews?limit={MAX_REVIEWS}", self.pulls_url());
        let resp = match self.auth(self.client.get(&reviews_url)).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(out),
        };
        let reviews: Vec<serde_json::Value> = match resp.json().await {
            Ok(v) => v,
            Err(_) => return Ok(out),
        };

        for review in reviews.into_iter().take(MAX_REVIEWS) {
            let Some(id) = review["id"].as_u64() else {
                continue;
            };
            let author = review["user"]["login"].as_str().map(String::from);
            let comments_url = format!("{}/reviews/{id}/comments", self.pulls_url());
            let resp = match self.auth(self.client.get(&comments_url)).send().await {
                Ok(r) if r.status().is_success() => r,
                _ => continue,
            };
            let comments: Vec<serde_json::Value> = match resp.json().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            for c in comments {
                if let Some(body) = c.get("body").and_then(|b| b.as_str()) {
                    out.push(super::ExistingComment {
                        body: body.to_string(),
                        author: author.clone(),
                    });
                }
            }
        }
        Ok(out)
    }

    async fn post_review(&self, draft: &ReviewDraft) -> Result<(), ForgeError> {
        let payload = review_payload(&self.commit_sha, draft);
        let url = format!("{}/reviews", self.pulls_url());
        let response = self
            .auth(self.client.post(&url))
            .json(&payload)
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(ForgeError::ApiError(format!(
                "review creation failed with HTTP {status}: {body}"
            )));
        }
        Ok(())
    }

    fn run_footer(&self) -> String {
        match &self.pipeline_url {
            Some(url) => format!("\n\n[View pipeline]({url})"),
            None => String::new(),
        }
    }
}

/// Build a Forgejo `CreatePullReviewOptions` body from a neutral draft.
pub fn review_payload(commit_id: &str, draft: &ReviewDraft) -> serde_json::Value {
    serde_json::json!({
        "event": event_str(draft.event),
        "body": draft.summary,
        "commit_id": commit_id,
        "comments": draft.comments.iter().map(comment_json).collect::<Vec<_>>(),
    })
}

fn event_str(event: super::ReviewEvent) -> &'static str {
    match event {
        super::ReviewEvent::Comment => "COMMENT",
        super::ReviewEvent::Approve => "APPROVED",
        super::ReviewEvent::RequestChanges => "REQUEST_CHANGES",
    }
}

/// Map a neutral inline comment to Forgejo's review-comment shape. nitpik
/// findings anchor to the new file via `new_position`; a range collapses to
/// its last line (Forgejo review comments are single-position).
fn comment_json(c: &InlineComment) -> serde_json::Value {
    let new_position = c.end_line.filter(|&e| e > c.line).unwrap_or(c.line);
    serde_json::json!({
        "path": c.path,
        "body": c.body,
        "new_position": new_position,
        "old_position": 0,
    })
}

fn require_env(env: &Env, name: &str) -> Result<String, ForgeError> {
    env.var(name)
        .map_err(|_| ForgeError::MissingEnvVar(name.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{Forge, ReviewEvent, Side, build_review_draft};
    use crate::models::finding::{Finding, Severity};

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
                agent: "correctness".to_string(),
                evidence: Vec::new(),
            },
            Finding {
                file: "src/lib.rs".to_string(),
                line: 20,
                end_line: Some(24),
                severity: Severity::Warning,
                title: "Style".to_string(),
                message: "Style issue".to_string(),
                suggestion: None,
                agent: "correctness".to_string(),
                evidence: Vec::new(),
            },
        ]
    }

    fn env_full() -> Env {
        Env::mock([
            ("CI_FORGE_URL", "https://codeberg.org"),
            ("CI_REPO_OWNER", "o"),
            ("CI_REPO_NAME", "r"),
            ("CI_COMMIT_PULL_REQUEST", "7"),
            ("CI_COMMIT_SHA", "abc123"),
            ("FORGEJO_TOKEN", "tok"),
        ])
    }

    #[test]
    fn from_env_builds_api_base_and_urls() {
        let forge = ForgejoForge::from_env(&env_full()).unwrap();
        assert_eq!(forge.api_base, "https://codeberg.org/api/v1");
        assert_eq!(
            forge.pulls_url(),
            "https://codeberg.org/api/v1/repos/o/r/pulls/7"
        );
        assert_eq!(forge.commit_sha, "abc123");
    }

    #[test]
    fn from_env_requires_each_var() {
        // Missing token (everything else present)
        let env = Env::mock([
            ("CI_FORGE_URL", "https://codeberg.org"),
            ("CI_REPO_OWNER", "o"),
            ("CI_REPO_NAME", "r"),
            ("CI_COMMIT_PULL_REQUEST", "7"),
            ("CI_COMMIT_SHA", "abc123"),
        ]);
        assert!(matches!(
            ForgejoForge::from_env(&env),
            Err(ForgeError::MissingEnvVar(_))
        ));
        // Non-numeric PR index
        let env = Env::mock([
            ("CI_FORGE_URL", "https://codeberg.org"),
            ("CI_REPO_OWNER", "o"),
            ("CI_REPO_NAME", "r"),
            ("CI_COMMIT_PULL_REQUEST", "nope"),
            ("CI_COMMIT_SHA", "abc123"),
            ("FORGEJO_TOKEN", "tok"),
        ]);
        assert!(matches!(
            ForgejoForge::from_env(&env),
            Err(ForgeError::NoPullRequest)
        ));
    }

    #[test]
    fn review_payload_carries_commit_and_inline_positions() {
        let f = sample_findings();
        let p = review_payload(
            "abc123",
            &build_review_draft(&f, &f, "", ReviewEvent::Comment),
        );
        assert_eq!(p["event"], "COMMENT");
        assert_eq!(p["commit_id"], "abc123");
        assert!(p["body"].as_str().unwrap().contains("2 findings"));
        let comments = p["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 2);
        // Single-line finding.
        assert_eq!(comments[0]["path"], "src/main.rs");
        assert_eq!(comments[0]["new_position"], 10);
        assert_eq!(comments[0]["old_position"], 0);
        // Range collapses to last line.
        assert_eq!(comments[1]["new_position"], 24);
    }

    #[test]
    fn left_and_event_mappings() {
        assert_eq!(event_str(ReviewEvent::Comment), "COMMENT");
        assert_eq!(event_str(ReviewEvent::Approve), "APPROVED");
        assert_eq!(event_str(ReviewEvent::RequestChanges), "REQUEST_CHANGES");
        // Side is irrelevant to Forgejo's new_position anchor, but the
        // neutral comment still carries it.
        let c = InlineComment {
            path: "a.rs".into(),
            line: 3,
            end_line: None,
            side: Side::Right,
            body: "x".into(),
        };
        assert_eq!(comment_json(&c)["new_position"], 3);
    }

    #[test]
    fn run_footer_uses_pipeline_url() {
        let env = Env::mock([
            ("CI_FORGE_URL", "https://codeberg.org"),
            ("CI_REPO_OWNER", "o"),
            ("CI_REPO_NAME", "r"),
            ("CI_COMMIT_PULL_REQUEST", "7"),
            ("CI_COMMIT_SHA", "abc123"),
            ("FORGEJO_TOKEN", "tok"),
            ("CI_PIPELINE_URL", "https://codeberg.org/o/r/pipelines/9"),
        ]);
        let forge = ForgejoForge::from_env(&env).unwrap();
        assert!(forge.run_footer().contains("/pipelines/9"));
    }
}
