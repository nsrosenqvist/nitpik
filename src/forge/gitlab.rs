//! GitLab forge adapter.
//!
//! Reads and posts a merge-request review via the GitLab REST API (v4):
//!
//! - read  — `GET /projects/{id}/merge_requests/{iid}` (MR metadata +
//!   `diff_refs`), `GET .../notes` (existing comments)
//! - write — `POST .../notes` (summary) + `POST .../discussions` (one per
//!   inline comment, with a full diff `position`)
//!
//! GitLab has no single "review" object like GitHub: a review is a summary
//! note plus a set of inline discussions. Each inline discussion needs a
//! `position` built from the MR's `diff_refs` (`base_sha` / `head_sha` /
//! `start_sha`) — an extra read GitHub doesn't require — which is why the
//! adapter fetches the MR before posting inline comments.
//!
//! **Support status:** implemented against the shared [`Forge`](super::Forge)
//! trait so the abstraction has two real backends, but **not yet
//! advertised** — pending live testing. The auth note below is the main
//! open item.
//!
//! # Required environment variables
//!
//! | Variable | Source |
//! |---|---|
//! | `CI_API_V4_URL` or `CI_SERVER_URL` | GitLab CI built-in |
//! | `CI_PROJECT_ID` | GitLab CI built-in |
//! | `CI_MERGE_REQUEST_IID` | GitLab CI built-in (merge-request pipelines) |
//! | `GITLAB_TOKEN` | User-provided (project/personal access token) |
//!
//! `CI_JOB_TOKEN` is accepted as a fallback but typically lacks the API
//! scope to post MR comments, so a `GITLAB_TOKEN` is recommended.
//! `CI_PIPELINE_URL` (run-link footer) is used when present.

use crate::env::Env;

use super::{ForgeError, InlineComment, PullRequest, ReviewDraft};

/// The diff SHAs a GitLab inline `position` is anchored against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRefs {
    pub base_sha: String,
    pub head_sha: String,
    pub start_sha: String,
}

/// GitLab forge adapter. Owns everything resolved from the environment.
pub struct GitlabForge {
    client: reqwest::Client,
    api_base: String,
    project_id: String,
    mr_iid: u64,
    /// (header-name, token) — `PRIVATE-TOKEN` for a personal/project token,
    /// `JOB-TOKEN` for the CI job token fallback.
    auth: (&'static str, String),
    pipeline_url: Option<String>,
}

impl GitlabForge {
    /// Construct from the environment. Returns an error if the API base,
    /// project, MR IID, or a token is missing — which is how
    /// [`super::detect`] decides GitLab is not the active forge.
    pub fn from_env(env: &Env) -> Result<Self, ForgeError> {
        let api_base = env
            .var("CI_API_V4_URL")
            .ok()
            .or_else(|| {
                env.var("CI_SERVER_URL")
                    .ok()
                    .map(|u| format!("{}/api/v4", u.trim_end_matches('/')))
            })
            .ok_or_else(|| ForgeError::MissingEnvVar("CI_API_V4_URL".into()))?;
        let api_base = api_base.trim_end_matches('/').to_string();

        let project_id = require_env(env, "CI_PROJECT_ID")?;
        let mr_iid_str = require_env(env, "CI_MERGE_REQUEST_IID")?;
        let mr_iid: u64 = mr_iid_str.parse().map_err(|_| ForgeError::NoPullRequest)?;

        let auth = if let Ok(t) = env.var("GITLAB_TOKEN") {
            ("PRIVATE-TOKEN", t)
        } else if let Ok(t) = env.var("CI_JOB_TOKEN") {
            ("JOB-TOKEN", t)
        } else {
            return Err(ForgeError::MissingEnvVar("GITLAB_TOKEN".into()));
        };

        let client = crate::http::build_client()
            .map_err(|e| ForgeError::ApiError(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            api_base,
            project_id,
            mr_iid,
            auth,
            pipeline_url: env.var("CI_PIPELINE_URL").ok(),
        })
    }

    fn auth_header(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header(self.auth.0, self.auth.1.clone())
    }

    fn mr_url(&self) -> String {
        format!(
            "{}/projects/{}/merge_requests/{}",
            self.api_base, self.project_id, self.mr_iid
        )
    }

    /// Fetch the MR's `diff_refs`, required to anchor inline discussions.
    async fn diff_refs(&self) -> Result<DiffRefs, ForgeError> {
        let resp = self
            .auth_header(self.client.get(self.mr_url()))
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ForgeError::ApiError(format!(
                "fetch MR failed with HTTP {}",
                resp.status()
            )));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        parse_diff_refs(&json)
            .ok_or_else(|| ForgeError::ApiError("MR response missing diff_refs".into()))
    }
}

#[async_trait::async_trait]
impl super::Forge for GitlabForge {
    async fn pull_request(&self) -> Result<PullRequest, ForgeError> {
        let resp = self
            .auth_header(self.client.get(self.mr_url()))
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(ForgeError::ApiError(format!(
                "fetch MR failed with HTTP {}",
                resp.status()
            )));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        Ok(PullRequest {
            number: self.mr_iid,
            title: json["title"].as_str().unwrap_or_default().to_string(),
            body: json["description"].as_str().unwrap_or_default().to_string(),
            head_sha: json["diff_refs"]["head_sha"]
                .as_str()
                .or_else(|| json["sha"].as_str())
                .unwrap_or_default()
                .to_string(),
            base_ref: json["target_branch"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            author: json["author"]["username"].as_str().map(String::from),
        })
    }

    async fn existing_review_comments(&self) -> Result<Vec<super::ExistingComment>, ForgeError> {
        const MAX_PAGES: u32 = 10;
        let mut out = Vec::new();

        for page in 1..=MAX_PAGES {
            let url = format!("{}/notes?per_page=100&page={page}", self.mr_url());
            let resp = self
                .auth_header(self.client.get(&url))
                .send()
                .await
                .map_err(|e| ForgeError::ApiError(e.to_string()))?;
            if !resp.status().is_success() {
                break;
            }
            let notes: Vec<serde_json::Value> = match resp.json().await {
                Ok(c) => c,
                Err(_) => break,
            };
            let page_len = notes.len();
            for n in notes {
                if let Some(body) = n.get("body").and_then(|b| b.as_str()) {
                    out.push(super::ExistingComment {
                        body: body.to_string(),
                        author: n["author"]["username"].as_str().map(String::from),
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
        // Inline discussions need the MR's diff SHAs — fetch once up front
        // (skip if there are no inline comments to anchor).
        let refs = if draft.comments.is_empty() {
            None
        } else {
            Some(self.diff_refs().await?)
        };

        // Summary as a plain MR note.
        let notes_url = format!("{}/notes", self.mr_url());
        let resp = self
            .auth_header(self.client.post(&notes_url))
            .json(&serde_json::json!({ "body": draft.summary }))
            .send()
            .await
            .map_err(|e| ForgeError::ApiError(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
            return Err(ForgeError::ApiError(format!(
                "summary note failed with HTTP {status}: {body}"
            )));
        }

        // Each inline comment as a positioned discussion.
        if let Some(refs) = refs {
            let discussions_url = format!("{}/discussions", self.mr_url());
            for c in &draft.comments {
                let payload = discussion_payload(&refs, c);
                let resp = self
                    .auth_header(self.client.post(&discussions_url))
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| ForgeError::ApiError(e.to_string()))?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
                    return Err(ForgeError::ApiError(format!(
                        "inline discussion on {} failed with HTTP {status}: {body}",
                        c.path
                    )));
                }
            }
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

/// Parse `diff_refs` out of an MR JSON response.
fn parse_diff_refs(mr: &serde_json::Value) -> Option<DiffRefs> {
    let d = mr.get("diff_refs")?;
    Some(DiffRefs {
        base_sha: d["base_sha"].as_str()?.to_string(),
        head_sha: d["head_sha"].as_str()?.to_string(),
        start_sha: d["start_sha"].as_str()?.to_string(),
    })
}

/// Build a GitLab discussion payload (body + inline `position`) from a
/// neutral comment. nitpik findings anchor to the new-file side, so the
/// position carries `new_path` + `new_line`; `old_path` mirrors the path
/// (GitLab requires both for a text position). A range collapses to its
/// last line — GitLab line-ranges are a later refinement.
pub fn discussion_payload(refs: &DiffRefs, c: &InlineComment) -> serde_json::Value {
    let new_line = c.end_line.filter(|&e| e > c.line).unwrap_or(c.line);
    serde_json::json!({
        "body": c.body,
        "position": {
            "base_sha": refs.base_sha,
            "head_sha": refs.head_sha,
            "start_sha": refs.start_sha,
            "position_type": "text",
            "new_path": c.path,
            "old_path": c.path,
            "new_line": new_line,
        }
    })
}

fn require_env(env: &Env, name: &str) -> Result<String, ForgeError> {
    env.var(name)
        .map_err(|_| ForgeError::MissingEnvVar(name.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{Forge, Side};

    fn refs() -> DiffRefs {
        DiffRefs {
            base_sha: "base".into(),
            head_sha: "head".into(),
            start_sha: "start".into(),
        }
    }

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
    fn parse_diff_refs_reads_all_three_shas() {
        let mr = serde_json::json!({
            "diff_refs": { "base_sha": "b", "head_sha": "h", "start_sha": "s" }
        });
        let r = parse_diff_refs(&mr).unwrap();
        assert_eq!(r.base_sha, "b");
        assert_eq!(r.head_sha, "h");
        assert_eq!(r.start_sha, "s");
    }

    #[test]
    fn parse_diff_refs_missing_is_none() {
        assert!(parse_diff_refs(&serde_json::json!({})).is_none());
    }

    #[test]
    fn discussion_payload_anchors_new_line_with_shas() {
        let p = discussion_payload(&refs(), &comment(12, None));
        assert_eq!(p["body"], "msg");
        let pos = &p["position"];
        assert_eq!(pos["position_type"], "text");
        assert_eq!(pos["new_path"], "src/app.rs");
        assert_eq!(pos["old_path"], "src/app.rs");
        assert_eq!(pos["new_line"], 12);
        assert_eq!(pos["base_sha"], "base");
        assert_eq!(pos["head_sha"], "head");
        assert_eq!(pos["start_sha"], "start");
    }

    #[test]
    fn discussion_payload_range_anchors_to_last_line() {
        let p = discussion_payload(&refs(), &comment(20, Some(24)));
        assert_eq!(p["position"]["new_line"], 24);
    }

    #[test]
    fn from_env_builds_api_base_from_server_url() {
        let env = Env::mock([
            ("CI_SERVER_URL", "https://gitlab.example.com"),
            ("CI_PROJECT_ID", "42"),
            ("CI_MERGE_REQUEST_IID", "7"),
            ("GITLAB_TOKEN", "tok"),
        ]);
        let forge = GitlabForge::from_env(&env).unwrap();
        assert_eq!(forge.api_base, "https://gitlab.example.com/api/v4");
        assert_eq!(
            forge.mr_url(),
            "https://gitlab.example.com/api/v4/projects/42/merge_requests/7"
        );
        assert_eq!(forge.auth.0, "PRIVATE-TOKEN");
    }

    #[test]
    fn from_env_prefers_api_v4_url_and_job_token_fallback() {
        let env = Env::mock([
            ("CI_API_V4_URL", "https://gitlab.com/api/v4"),
            ("CI_PROJECT_ID", "1"),
            ("CI_MERGE_REQUEST_IID", "2"),
            ("CI_JOB_TOKEN", "jt"),
        ]);
        let forge = GitlabForge::from_env(&env).unwrap();
        assert_eq!(forge.api_base, "https://gitlab.com/api/v4");
        assert_eq!(forge.auth, ("JOB-TOKEN", "jt".to_string()));
    }

    #[test]
    fn from_env_requires_project_mr_and_token() {
        // Missing everything
        assert!(matches!(
            GitlabForge::from_env(&Env::mock(Vec::<(&str, &str)>::new())),
            Err(ForgeError::MissingEnvVar(_))
        ));
        // Missing token
        let env = Env::mock([
            ("CI_API_V4_URL", "https://gitlab.com/api/v4"),
            ("CI_PROJECT_ID", "1"),
            ("CI_MERGE_REQUEST_IID", "2"),
        ]);
        assert!(matches!(
            GitlabForge::from_env(&env),
            Err(ForgeError::MissingEnvVar(_))
        ));
        // Non-numeric MR IID
        let env = Env::mock([
            ("CI_API_V4_URL", "https://gitlab.com/api/v4"),
            ("CI_PROJECT_ID", "1"),
            ("CI_MERGE_REQUEST_IID", "not-a-number"),
            ("GITLAB_TOKEN", "t"),
        ]);
        assert!(matches!(
            GitlabForge::from_env(&env),
            Err(ForgeError::NoPullRequest)
        ));
    }

    #[test]
    fn run_footer_uses_pipeline_url() {
        let env = Env::mock([
            ("CI_API_V4_URL", "https://gitlab.com/api/v4"),
            ("CI_PROJECT_ID", "1"),
            ("CI_MERGE_REQUEST_IID", "2"),
            ("GITLAB_TOKEN", "t"),
            ("CI_PIPELINE_URL", "https://gitlab.com/o/r/-/pipelines/9"),
        ]);
        let forge = GitlabForge::from_env(&env).unwrap();
        assert!(forge.run_footer().contains("/-/pipelines/9"));
    }
}
