//! Bitbucket Cloud forge adapter.
//!
//! Reads and posts a pull-request review via the Bitbucket Cloud REST API
//! (2.0):
//!
//! - read  — `GET /pullrequests/{id}` (PR metadata), `GET .../comments`
//!   (existing comments, for cross-run dedup)
//! - write — `POST .../comments` for the summary, then one `POST .../comments`
//!   per inline finding (`inline: { path, to }`)
//!
//! Bitbucket has no single "review" object: a review is a summary comment
//! plus a set of inline comments. nitpik findings anchor to the new-file
//! side, so each inline comment positions on `inline.to` (the destination
//! line); a range collapses to its last line.
//!
//! **Support status:** implemented against the shared [`Forge`](super::Forge)
//! trait so PR-native review isn't GitHub-only, but **not yet advertised** —
//! pending live testing.
//!
//! # Required environment variables
//!
//! | Variable | Source |
//! |---|---|
//! | `BITBUCKET_WORKSPACE` | Bitbucket Pipelines built-in |
//! | `BITBUCKET_REPO_SLUG` | Bitbucket Pipelines built-in |
//! | `BITBUCKET_PR_ID` | Bitbucket Pipelines built-in (PR-triggered pipelines) |
//! | `BITBUCKET_TOKEN` | User-provided access token (Bearer) |
//!
//! Unlike the commit-level Code Insights output (which can use the in-Pipelines
//! proxy), posting PR comments needs a real bearer token — a repository,
//! project, or workspace access token with pull-request write scope.

use crate::env::Env;

use super::{ForgeError, InlineComment, PullRequest, ReviewDraft};

const API_ROOT: &str = "https://api.bitbucket.org/2.0/repositories";

/// Bitbucket Cloud forge adapter. Owns everything resolved from the environment.
pub struct BitbucketForge {
    client: reqwest::Client,
    workspace: String,
    repo_slug: String,
    pr_id: u64,
    token: String,
    build_number: Option<String>,
}

impl BitbucketForge {
    /// Construct from the environment. Returns an error if the workspace,
    /// repo, PR id, or token is missing — which is how [`super::detect`]
    /// decides Bitbucket is not the active forge.
    pub fn from_env(env: &Env) -> Result<Self, ForgeError> {
        let workspace = require_env(env, "BITBUCKET_WORKSPACE")?;
        let repo_slug = require_env(env, "BITBUCKET_REPO_SLUG")?;
        let pr_id_str = require_env(env, "BITBUCKET_PR_ID")?;
        let pr_id: u64 = pr_id_str.parse().map_err(|_| ForgeError::NoPullRequest)?;
        let token = require_env(env, "BITBUCKET_TOKEN")?;

        let client = crate::http::build_client()
            .map_err(|e| ForgeError::ApiError(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            workspace,
            repo_slug,
            pr_id,
            token,
            build_number: env.var("BITBUCKET_BUILD_NUMBER").ok(),
        })
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.bearer_auth(&self.token)
    }

    fn pr_url(&self) -> String {
        format!(
            "{API_ROOT}/{}/{}/pullrequests/{}",
            self.workspace, self.repo_slug, self.pr_id
        )
    }
}

#[async_trait::async_trait]
impl super::Forge for BitbucketForge {
    async fn pull_request(&self) -> Result<PullRequest, ForgeError> {
        let resp = self
            .auth(self.client.get(self.pr_url()))
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
            number: self.pr_id,
            title: json["title"].as_str().unwrap_or_default().to_string(),
            body: json["description"].as_str().unwrap_or_default().to_string(),
            head_sha: json["source"]["commit"]["hash"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            base_ref: json["destination"]["branch"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            author: json["author"]["nickname"]
                .as_str()
                .or_else(|| json["author"]["display_name"].as_str())
                .map(String::from),
        })
    }

    async fn existing_review_comments(&self) -> Result<Vec<super::ExistingComment>, ForgeError> {
        const MAX_PAGES: u32 = 10;
        let mut out = Vec::new();
        let mut url = format!("{}/comments?pagelen=100", self.pr_url());

        for _ in 0..MAX_PAGES {
            let resp = self
                .auth(self.client.get(&url))
                .send()
                .await
                .map_err(|e| ForgeError::ApiError(e.to_string()))?;
            if !resp.status().is_success() {
                break;
            }
            let page: serde_json::Value = match resp.json().await {
                Ok(p) => p,
                Err(_) => break,
            };
            if let Some(values) = page["values"].as_array() {
                for c in values {
                    if let Some(body) = c["content"]["raw"].as_str() {
                        out.push(super::ExistingComment {
                            body: body.to_string(),
                            author: c["user"]["nickname"]
                                .as_str()
                                .or_else(|| c["user"]["display_name"].as_str())
                                .map(String::from),
                        });
                    }
                }
            }
            // Bitbucket paginates with an absolute `next` URL; follow it.
            match page["next"].as_str() {
                Some(next) => url = next.to_string(),
                None => break,
            }
        }
        Ok(out)
    }

    async fn post_review(&self, draft: &ReviewDraft) -> Result<(), ForgeError> {
        let comments_url = format!("{}/comments", self.pr_url());

        // Summary as a top-level PR comment.
        let resp = self
            .auth(self.client.post(&comments_url))
            .json(&serde_json::json!({ "content": { "raw": draft.summary } }))
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
            return Err(ForgeError::ApiError(format!(
                "summary comment failed with HTTP {status}: {body}"
            )));
        }

        // Each finding as an inline comment.
        for c in &draft.comments {
            let resp = self
                .auth(self.client.post(&comments_url))
                .json(&inline_comment_payload(c))
                .send()
                .await
                .map_err(|e| ForgeError::ApiError(e.to_string()))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
                return Err(ForgeError::ApiError(format!(
                    "inline comment on {} failed with HTTP {status}: {body}",
                    c.path
                )));
            }
        }
        Ok(())
    }

    fn run_footer(&self) -> String {
        match &self.build_number {
            Some(n) => format!(
                "\n\n[View pipeline](https://bitbucket.org/{}/{}/pipelines/results/{n})",
                self.workspace, self.repo_slug
            ),
            None => String::new(),
        }
    }
}

/// Build a Bitbucket inline-comment payload from a neutral comment. nitpik
/// findings anchor to the new-file side, so the position uses `inline.to`
/// (the destination line). A range collapses to its last line — Bitbucket
/// inline comments anchor to a single line.
pub fn inline_comment_payload(c: &InlineComment) -> serde_json::Value {
    let line = c.end_line.filter(|&e| e > c.line).unwrap_or(c.line);
    serde_json::json!({
        "content": { "raw": c.body },
        "inline": { "path": c.path, "to": line },
    })
}

fn require_env(env: &Env, name: &str) -> Result<String, ForgeError> {
    env.var(name).map_err(|_| ForgeError::MissingEnvVar(name.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{Forge, Side};

    fn comment(line: u32, end: Option<u32>) -> InlineComment {
        InlineComment {
            path: "src/app.rs".into(),
            line,
            end_line: end,
            side: Side::Right,
            body: "msg".into(),
        }
    }

    #[test]
    fn inline_payload_anchors_to_new_line() {
        let p = inline_comment_payload(&comment(12, None));
        assert_eq!(p["content"]["raw"], "msg");
        assert_eq!(p["inline"]["path"], "src/app.rs");
        assert_eq!(p["inline"]["to"], 12);
    }

    #[test]
    fn inline_payload_range_anchors_to_last_line() {
        let p = inline_comment_payload(&comment(20, Some(24)));
        assert_eq!(p["inline"]["to"], 24);
    }

    #[test]
    fn from_env_builds_pr_url() {
        let env = Env::mock([
            ("BITBUCKET_WORKSPACE", "acme"),
            ("BITBUCKET_REPO_SLUG", "widget"),
            ("BITBUCKET_PR_ID", "7"),
            ("BITBUCKET_TOKEN", "tok"),
        ]);
        let forge = BitbucketForge::from_env(&env).unwrap();
        assert_eq!(
            forge.pr_url(),
            "https://api.bitbucket.org/2.0/repositories/acme/widget/pullrequests/7"
        );
    }

    #[test]
    fn from_env_requires_all_fields() {
        assert!(matches!(
            BitbucketForge::from_env(&Env::mock(Vec::<(&str, &str)>::new())),
            Err(ForgeError::MissingEnvVar(_))
        ));
        // Present everything but the token.
        let env = Env::mock([
            ("BITBUCKET_WORKSPACE", "acme"),
            ("BITBUCKET_REPO_SLUG", "widget"),
            ("BITBUCKET_PR_ID", "7"),
        ]);
        assert!(matches!(
            BitbucketForge::from_env(&env),
            Err(ForgeError::MissingEnvVar(_))
        ));
        // Non-numeric PR id.
        let env = Env::mock([
            ("BITBUCKET_WORKSPACE", "acme"),
            ("BITBUCKET_REPO_SLUG", "widget"),
            ("BITBUCKET_PR_ID", "not-a-number"),
            ("BITBUCKET_TOKEN", "tok"),
        ]);
        assert!(matches!(
            BitbucketForge::from_env(&env),
            Err(ForgeError::NoPullRequest)
        ));
    }

    #[test]
    fn run_footer_links_pipeline_when_build_number_present() {
        let env = Env::mock([
            ("BITBUCKET_WORKSPACE", "acme"),
            ("BITBUCKET_REPO_SLUG", "widget"),
            ("BITBUCKET_PR_ID", "7"),
            ("BITBUCKET_TOKEN", "tok"),
            ("BITBUCKET_BUILD_NUMBER", "42"),
        ]);
        let forge = BitbucketForge::from_env(&env).unwrap();
        assert!(forge.run_footer().contains("/pipelines/results/42"));
    }

    #[test]
    fn run_footer_empty_without_build_number() {
        let env = Env::mock([
            ("BITBUCKET_WORKSPACE", "acme"),
            ("BITBUCKET_REPO_SLUG", "widget"),
            ("BITBUCKET_PR_ID", "7"),
            ("BITBUCKET_TOKEN", "tok"),
        ]);
        let forge = BitbucketForge::from_env(&env).unwrap();
        assert!(forge.run_footer().is_empty());
    }
}
