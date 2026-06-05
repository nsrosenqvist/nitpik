//! GitHub forge adapter.
//!
//! Reads and posts a PR review via the GitHub REST API using the
//! workflow's built-in `GITHUB_TOKEN` (no GitHub App required):
//!
//! - read  — `GET /repos/{owner}/{repo}/pulls/{n}` (PR metadata),
//!   `GET /repos/{owner}/{repo}/pulls/{n}/comments` (existing comments)
//! - write — `POST /repos/{owner}/{repo}/pulls/{n}/reviews`
//!
//! Inline comments anchor to the `RIGHT` (new-file) side; multi-line
//! findings use GitHub's `start_line`/`line` range form. `commit_id` is
//! intentionally omitted so GitHub anchors to the PR's latest commit,
//! avoiding the merge-ref-vs-head-SHA ambiguity on `pull_request` events.
//!
//! # Required environment variables
//!
//! | Variable | Source |
//! |---|---|
//! | `GITHUB_TOKEN` | Actions built-in (needs `pull-requests: write`) |
//! | `GITHUB_REPOSITORY` | Actions built-in (`owner/repo`) |
//! | `GITHUB_REF` or `GITHUB_EVENT_PATH` | Actions built-in (PR number) |
//!
//! `GITHUB_API_URL` (Enterprise) and `GITHUB_SERVER_URL` / `GITHUB_RUN_ID`
//! (run-link footer) are used when present.

use crate::env::Env;

use super::{ForgeError, InlineComment, PullRequest, ReviewDraft, Side};

/// GitHub forge adapter. Holds everything resolved from the environment at
/// construction so the adapter owns no borrow of [`Env`].
pub struct GithubForge {
    client: reqwest::Client,
    api_base: String,
    owner: String,
    repo: String,
    pr_number: u64,
    token: String,
    /// Run-link footer parts (`server_url`, `repository`, `run_id`), if set.
    run_link: Option<(String, String, String)>,
}

impl GithubForge {
    /// Construct from the environment. Returns an error if the token,
    /// repository, or a resolvable PR number is missing — which is how
    /// [`super::detect`] decides GitHub is not the active forge.
    pub fn from_env(env: &Env) -> Result<Self, ForgeError> {
        let token = require_env(env, "GITHUB_TOKEN")?;
        let repository = require_env(env, "GITHUB_REPOSITORY")?;
        let (owner, repo) = repository
            .split_once('/')
            .ok_or_else(|| ForgeError::InvalidRepository(repository.clone()))?;
        let pr_number = resolve_pr_number(env)?;

        let api_base = env
            .var("GITHUB_API_URL")
            .unwrap_or_else(|_| "https://api.github.com".to_string());
        let api_base = api_base.trim_end_matches('/').to_string();

        let client = crate::http::build_client()
            .map_err(|e| ForgeError::ApiError(format!("failed to build HTTP client: {e}")))?;

        let run_link = match (
            env.var("GITHUB_SERVER_URL"),
            env.var("GITHUB_REPOSITORY"),
            env.var("GITHUB_RUN_ID"),
        ) {
            (Ok(s), Ok(r), Ok(id)) => Some((s, r, id)),
            _ => None,
        };

        Ok(Self {
            client,
            api_base,
            owner: owner.to_string(),
            repo: repo.to_string(),
            pr_number,
            token,
            run_link,
        })
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }
}

#[async_trait::async_trait]
impl super::Forge for GithubForge {
    async fn pull_request(&self) -> Result<PullRequest, ForgeError> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.api_base, self.owner, self.repo, self.pr_number
        );
        let resp = self
            .auth(self.client.get(&url))
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
            number: self.pr_number,
            title: json["title"].as_str().unwrap_or_default().to_string(),
            body: json["body"].as_str().unwrap_or_default().to_string(),
            head_sha: json["head"]["sha"].as_str().unwrap_or_default().to_string(),
            base_ref: json["base"]["ref"].as_str().unwrap_or_default().to_string(),
            author: json["user"]["login"].as_str().map(String::from),
        })
    }

    async fn existing_review_comments(&self) -> Result<Vec<super::ExistingComment>, ForgeError> {
        const MAX_PAGES: u32 = 10;
        let mut out = Vec::new();

        for page in 1..=MAX_PAGES {
            let url = format!(
                "{}/repos/{}/{}/pulls/{}/comments?per_page=100&page={page}",
                self.api_base, self.owner, self.repo, self.pr_number
            );
            let resp = self
                .auth(self.client.get(&url))
                .send()
                .await
                .map_err(|e| ForgeError::ApiError(e.to_string()))?;
            if !resp.status().is_success() {
                break;
            }
            let comments: Vec<serde_json::Value> = match resp.json().await {
                Ok(c) => c,
                Err(_) => break,
            };
            let page_len = comments.len();
            for c in comments {
                if let Some(body) = c.get("body").and_then(|b| b.as_str()) {
                    out.push(super::ExistingComment {
                        body: body.to_string(),
                        author: c["user"]["login"].as_str().map(String::from),
                    });
                }
            }
            if page_len < 100 {
                break;
            }
        }
        Ok(out)
    }

    async fn post_review(&self, draft: &ReviewDraft) -> Result<(), ForgeError> {
        let payload = review_payload(draft);
        let url = format!(
            "{}/repos/{}/{}/pulls/{}/reviews",
            self.api_base, self.owner, self.repo, self.pr_number
        );
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

    async fn open_review_threads(&self) -> Result<Vec<super::ReviewThread>, ForgeError> {
        // Thread resolution state lives only in GraphQL, not REST.
        let body = review_threads_query(&self.owner, &self.repo, self.pr_number);
        let resp = self
            .auth(self.client.post(self.graphql_url()))
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ForgeError::ApiError(format!(
                "review-threads query failed with HTTP {}",
                resp.status()
            )));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        Ok(parse_review_threads(&json))
    }

    async fn reply_and_resolve(
        &self,
        thread: &super::ReviewThread,
        reply: &str,
    ) -> Result<(), ForgeError> {
        // Reply to the root comment (REST), then resolve the thread (GraphQL).
        if let Some(root_id) = thread.root_comment_id {
            let url = format!(
                "{}/repos/{}/{}/pulls/{}/comments/{root_id}/replies",
                self.api_base, self.owner, self.repo, self.pr_number
            );
            let resp = self
                .auth(self.client.post(&url))
                .json(&serde_json::json!({ "body": reply }))
                .send()
                .await
                .map_err(|e| ForgeError::ApiError(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(ForgeError::ApiError(format!(
                    "reply to comment {root_id} failed with HTTP {}",
                    resp.status()
                )));
            }
        }

        let resp = self
            .auth(self.client.post(self.graphql_url()))
            .json(&resolve_thread_mutation(&thread.id))
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ForgeError::ApiError(format!(
                "resolve thread {} failed with HTTP {}",
                thread.id,
                resp.status()
            )));
        }
        Ok(())
    }

    fn run_footer(&self) -> String {
        match &self.run_link {
            Some((server, repo, run_id)) => {
                format!("\n\n[View run]({server}/{repo}/actions/runs/{run_id})")
            }
            None => String::new(),
        }
    }
}

impl GithubForge {
    /// GraphQL endpoint for this host. github.com exposes it at
    /// `/graphql`; GitHub Enterprise (`…/api/v3` REST) at `…/api/graphql`.
    fn graphql_url(&self) -> String {
        match self.api_base.strip_suffix("/api/v3") {
            Some(host) => format!("{host}/api/graphql"),
            None => format!("{}/graphql", self.api_base),
        }
    }
}

const REVIEW_THREADS_QUERY: &str = r#"
query($owner:String!,$repo:String!,$pr:Int!) {
  repository(owner:$owner,name:$repo) {
    pullRequest(number:$pr) {
      reviewThreads(first:100) {
        nodes {
          id isResolved isOutdated
          comments(first:1) {
            nodes { databaseId body path line }
          }
        }
      }
    }
  }
}"#;

/// Build the GraphQL request body to list a PR's review threads.
pub fn review_threads_query(owner: &str, repo: &str, pr: u64) -> serde_json::Value {
    serde_json::json!({
        "query": REVIEW_THREADS_QUERY,
        "variables": { "owner": owner, "repo": repo, "pr": pr },
    })
}

/// Build the GraphQL mutation body to resolve a review thread.
pub fn resolve_thread_mutation(thread_id: &str) -> serde_json::Value {
    serde_json::json!({
        "query": "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}",
        "variables": { "id": thread_id },
    })
}

/// Parse the review-threads GraphQL response into neutral [`ReviewThread`]s,
/// keeping only unresolved threads and lifting the root comment's fields.
fn parse_review_threads(json: &serde_json::Value) -> Vec<super::ReviewThread> {
    let nodes = json["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"].as_array();
    let Some(nodes) = nodes else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for t in nodes {
        if t["isResolved"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(id) = t["id"].as_str() else { continue };
        let root = &t["comments"]["nodes"][0];
        let body = root["body"].as_str().unwrap_or_default().to_string();
        let fingerprint = super::extract_markers(std::iter::once(body.as_str()))
            .into_iter()
            .next();
        out.push(super::ReviewThread {
            id: id.to_string(),
            root_comment_id: root["databaseId"].as_u64(),
            fingerprint,
            path: root["path"].as_str().unwrap_or_default().to_string(),
            line: root["line"].as_u64().map(|l| l as u32),
            outdated: t["isOutdated"].as_bool().unwrap_or(false),
            body,
        });
    }
    out
}

/// Build the `createReview` request body from a neutral draft.
pub fn review_payload(draft: &ReviewDraft) -> serde_json::Value {
    serde_json::json!({
        "event": event_str(draft.event),
        "body": draft.summary,
        "comments": draft.comments.iter().map(comment_json).collect::<Vec<_>>(),
    })
}

fn event_str(event: super::ReviewEvent) -> &'static str {
    match event {
        super::ReviewEvent::Comment => "COMMENT",
        super::ReviewEvent::Approve => "APPROVE",
        super::ReviewEvent::RequestChanges => "REQUEST_CHANGES",
    }
}

fn side_str(side: Side) -> &'static str {
    match side {
        Side::Right => "RIGHT",
        Side::Left => "LEFT",
    }
}

/// Map a neutral inline comment to GitHub's review-comment shape. A range
/// (`end_line` past `line`) uses GitHub's `start_line`/`line` form, where
/// `line` is the *end* of the range.
fn comment_json(c: &InlineComment) -> serde_json::Value {
    let side = side_str(c.side);
    let (line, start_line) = match c.end_line {
        Some(end) if end > c.line => (end, Some(c.line)),
        _ => (c.line, None),
    };
    let mut json = serde_json::json!({
        "path": c.path,
        "side": side,
        "line": line,
        "body": c.body,
    });
    if let Some(start) = start_line {
        json["start_line"] = serde_json::json!(start);
        json["start_side"] = serde_json::json!(side);
    }
    json
}

fn require_env(env: &Env, name: &str) -> Result<String, ForgeError> {
    env.var(name)
        .map_err(|_| ForgeError::MissingEnvVar(name.into()))
}

/// Resolve the PR number from `GITHUB_REF` (`refs/pull/<n>/merge`) or the
/// event payload at `GITHUB_EVENT_PATH`. Three payload shapes are handled:
///
/// - `pull_request` events — `pull_request.number`,
/// - `issue_comment` events (e.g. `@nitpik review`) and `repository_dispatch`
///   — `issue.number`, but only when `issue.pull_request` is present (so a
///   comment on a plain issue is *not* mistaken for a PR),
/// - a top-level `number` as a last resort.
pub fn resolve_pr_number(env: &Env) -> Result<u64, ForgeError> {
    if let Ok(gh_ref) = env.var("GITHUB_REF")
        && let Some(n) = gh_ref
            .strip_prefix("refs/pull/")
            .and_then(|rest| rest.split('/').next())
            .and_then(|n| n.parse::<u64>().ok())
    {
        return Ok(n);
    }

    if let Ok(event_path) = env.var("GITHUB_EVENT_PATH")
        && let Ok(raw) = std::fs::read_to_string(&event_path)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        let n = json
            .get("pull_request")
            .and_then(|pr| pr.get("number"))
            .or_else(|| {
                json.get("issue")
                    .filter(|issue| issue.get("pull_request").is_some())
                    .and_then(|issue| issue.get("number"))
            })
            // repository_dispatch (e.g. the GitHub App firing `nitpik-review`):
            // GITHUB_REF is the default branch, and the PR number rides in
            // client_payload.pr — no pull_request/issue object in the payload.
            .or_else(|| {
                json.get("client_payload")
                    .and_then(|cp| cp.get("pr").or_else(|| cp.get("number")))
            })
            .or_else(|| json.get("number"))
            .and_then(|n| n.as_u64());
        if let Some(n) = n {
            return Ok(n);
        }
    }

    Err(ForgeError::NoPullRequest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{InlineComment, ReviewEvent, build_review_draft};
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

    fn payload(findings: &[Finding]) -> serde_json::Value {
        review_payload(&build_review_draft(
            findings,
            findings,
            "",
            ReviewEvent::Comment,
        ))
    }

    #[test]
    fn request_changes_event_maps_to_request_changes() {
        let p = review_payload(&build_review_draft(
            &sample_findings(),
            &sample_findings(),
            "",
            ReviewEvent::RequestChanges,
        ));
        assert_eq!(p["event"], "REQUEST_CHANGES");
    }

    #[test]
    fn payload_is_comment_review_with_all_comments() {
        let p = payload(&sample_findings());
        assert_eq!(p["event"], "COMMENT");
        assert!(p["body"].as_str().unwrap().contains("2 findings"));
        assert_eq!(p["comments"].as_array().unwrap().len(), 2);
        // commit_id is intentionally omitted (GitHub defaults to latest).
        assert!(p.get("commit_id").is_none());
    }

    #[test]
    fn single_line_comment_uses_line_and_right_side() {
        let p = payload(&sample_findings());
        let c = &p["comments"][0];
        assert_eq!(c["path"], "src/main.rs");
        assert_eq!(c["line"], 10);
        assert_eq!(c["side"], "RIGHT");
        assert!(c.get("start_line").is_none());
    }

    #[test]
    fn multi_line_comment_uses_start_line_range() {
        let p = payload(&sample_findings());
        let c = &p["comments"][1];
        assert_eq!(c["line"], 24);
        assert_eq!(c["start_line"], 20);
        assert_eq!(c["start_side"], "RIGHT");
        assert_eq!(c["side"], "RIGHT");
    }

    #[test]
    fn left_side_comment_maps_to_left() {
        let c = InlineComment {
            path: "a.rs".into(),
            line: 3,
            end_line: None,
            side: Side::Left,
            body: "x".into(),
        };
        let j = comment_json(&c);
        assert_eq!(j["side"], "LEFT");
    }

    #[test]
    fn event_strings() {
        assert_eq!(event_str(ReviewEvent::Comment), "COMMENT");
        assert_eq!(event_str(ReviewEvent::Approve), "APPROVE");
        assert_eq!(event_str(ReviewEvent::RequestChanges), "REQUEST_CHANGES");
    }

    #[test]
    fn resolve_pr_number_from_github_ref() {
        let env = Env::mock([("GITHUB_REF", "refs/pull/123/merge")]);
        assert_eq!(resolve_pr_number(&env).unwrap(), 123);
    }

    #[test]
    fn resolve_pr_number_from_event_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        std::fs::write(&path, r#"{"pull_request":{"number":77}}"#).unwrap();
        let env = Env::mock([("GITHUB_EVENT_PATH", path.to_str().unwrap())]);
        assert_eq!(resolve_pr_number(&env).unwrap(), 77);
    }

    #[test]
    fn resolve_pr_number_from_issue_comment_payload() {
        // issue_comment events (e.g. `@nitpik review`) carry the PR number in
        // issue.number, with issue.pull_request present to mark it as a PR.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        std::fs::write(
            &path,
            r#"{"issue":{"number":88,"pull_request":{"url":"https://api.github.com/…/pulls/88"}}}"#,
        )
        .unwrap();
        let env = Env::mock([("GITHUB_EVENT_PATH", path.to_str().unwrap())]);
        assert_eq!(resolve_pr_number(&env).unwrap(), 88);
    }

    #[test]
    fn resolve_pr_number_ignores_plain_issue_comment() {
        // A comment on a non-PR issue has no issue.pull_request → not a PR.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        std::fs::write(&path, r#"{"issue":{"number":88}}"#).unwrap();
        let env = Env::mock([("GITHUB_EVENT_PATH", path.to_str().unwrap())]);
        assert!(matches!(
            resolve_pr_number(&env),
            Err(ForgeError::NoPullRequest)
        ));
    }

    #[test]
    fn resolve_pr_number_from_repository_dispatch_payload() {
        // The GitHub App fires repository_dispatch with the PR number in
        // client_payload.pr; GITHUB_REF points at the default branch, so the
        // refs/pull/N/merge fast path doesn't apply.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        std::fs::write(
            &path,
            r#"{"action":"nitpik-review","client_payload":{"pr":42,"force":true}}"#,
        )
        .unwrap();
        let env = Env::mock([
            ("GITHUB_REF", "refs/heads/main"),
            ("GITHUB_EVENT_PATH", path.to_str().unwrap()),
        ]);
        assert_eq!(resolve_pr_number(&env).unwrap(), 42);
    }

    #[test]
    fn resolve_pr_number_missing_errors() {
        let env = Env::mock([("GITHUB_REF", "refs/heads/main")]);
        assert!(matches!(
            resolve_pr_number(&env),
            Err(ForgeError::NoPullRequest)
        ));
    }

    #[test]
    fn from_env_requires_token_repo_and_pr() {
        // Missing token
        let env = Env::mock([
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_REF", "refs/pull/1/merge"),
        ]);
        assert!(matches!(
            GithubForge::from_env(&env),
            Err(ForgeError::MissingEnvVar(_))
        ));
        // Bad repo shape
        let env = Env::mock([
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "no-slash"),
            ("GITHUB_REF", "refs/pull/1/merge"),
        ]);
        assert!(matches!(
            GithubForge::from_env(&env),
            Err(ForgeError::InvalidRepository(_))
        ));
        // No PR resolvable
        let env = Env::mock([
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_REF", "refs/heads/main"),
        ]);
        assert!(matches!(
            GithubForge::from_env(&env),
            Err(ForgeError::NoPullRequest)
        ));
        // Complete
        let env = Env::mock([
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_REF", "refs/pull/5/merge"),
        ]);
        let forge = GithubForge::from_env(&env).unwrap();
        assert_eq!(forge.pr_number, 5);
        assert_eq!(forge.owner, "o");
        assert_eq!(forge.repo, "r");
    }

    #[test]
    fn review_threads_query_carries_variables() {
        let q = review_threads_query("o", "r", 42);
        assert!(q["query"].as_str().unwrap().contains("reviewThreads"));
        assert_eq!(q["variables"]["owner"], "o");
        assert_eq!(q["variables"]["repo"], "r");
        assert_eq!(q["variables"]["pr"], 42);
    }

    #[test]
    fn resolve_mutation_carries_thread_id() {
        let m = resolve_thread_mutation("THREAD_abc");
        assert!(m["query"].as_str().unwrap().contains("resolveReviewThread"));
        assert_eq!(m["variables"]["id"], "THREAD_abc");
    }

    #[test]
    fn parse_review_threads_skips_resolved_and_lifts_root() {
        let json = serde_json::json!({
          "data": { "repository": { "pullRequest": { "reviewThreads": { "nodes": [
            {
              "id": "T1", "isResolved": false, "isOutdated": true,
              "comments": { "nodes": [
                { "databaseId": 555, "body": "🔴 Bug\n\n<!-- nitpik:abc123 -->", "path": "db.py", "line": 6 }
              ]}
            },
            {
              "id": "T2", "isResolved": true, "isOutdated": false,
              "comments": { "nodes": [ { "databaseId": 777, "body": "old", "path": "x.py", "line": 1 } ]}
            }
          ]}}}}
        });
        let threads = parse_review_threads(&json);
        assert_eq!(threads.len(), 1); // resolved T2 dropped
        let t = &threads[0];
        assert_eq!(t.id, "T1");
        assert_eq!(t.root_comment_id, Some(555));
        assert_eq!(t.fingerprint.as_deref(), Some("abc123"));
        assert_eq!(t.path, "db.py");
        assert_eq!(t.line, Some(6));
        assert!(t.outdated);
    }

    #[test]
    fn parse_review_threads_empty_on_missing_data() {
        assert!(parse_review_threads(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn graphql_url_for_dotcom_and_enterprise() {
        let env = Env::mock([
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_REF", "refs/pull/5/merge"),
        ]);
        let forge = GithubForge::from_env(&env).unwrap();
        assert_eq!(forge.graphql_url(), "https://api.github.com/graphql");

        let env = Env::mock([
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_REF", "refs/pull/5/merge"),
            ("GITHUB_API_URL", "https://ghe.example.com/api/v3"),
        ]);
        let forge = GithubForge::from_env(&env).unwrap();
        assert_eq!(forge.graphql_url(), "https://ghe.example.com/api/graphql");
    }

    #[test]
    fn run_footer_present_when_env_set() {
        use crate::forge::Forge;
        let env = Env::mock([
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_REF", "refs/pull/5/merge"),
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_RUN_ID", "42"),
        ]);
        let forge = GithubForge::from_env(&env).unwrap();
        assert!(
            forge
                .run_footer()
                .contains("https://github.com/o/r/actions/runs/42")
        );
    }
}
