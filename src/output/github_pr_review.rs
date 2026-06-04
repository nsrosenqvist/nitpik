//! GitHub Pull Request review formatter and publisher.
//!
//! Posts findings as a single PR review with inline comments via the
//! GitHub REST API (`POST /repos/{owner}/{repo}/pulls/{n}/reviews`). The
//! synchronous `format()` output is the JSON payload, for inspection or
//! piping.
//!
//! Unlike the [`github`](crate::output::github) formatter — which only
//! emits ephemeral Actions workflow annotations — this posts a durable,
//! reviewable PR review using the workflow's built-in `GITHUB_TOKEN` (no
//! GitHub App required). Findings reaching this layer are already
//! restricted to changed lines by the orchestrator's diff-scope filter,
//! so each anchors to a commentable line on the `RIGHT` (new-file) side.
//!
//! # Required environment variables
//!
//! | Variable | Source |
//! |---|---|
//! | `GITHUB_TOKEN` | Actions built-in (needs `pull-requests: write`) |
//! | `GITHUB_REPOSITORY` | Actions built-in (`owner/repo`) |
//! | `GITHUB_REF` or `GITHUB_EVENT_PATH` | Actions built-in (to find the PR number) |
//!
//! `GITHUB_API_URL` (GitHub Enterprise) and `GITHUB_SERVER_URL` /
//! `GITHUB_RUN_ID` (run-link footer) are used when present.

use std::collections::HashSet;

use crate::env::Env;
use crate::models::finding::Finding;
use crate::output::{OutputFormatter, OutputPublisher};
use thiserror::Error;

/// Stable, line-independent fingerprint of a finding, used to recognize
/// the same issue across PR pushes (line numbers shift; `path | title |
/// evidence` does not). Hashed so it embeds cleanly in a comment marker.
fn fingerprint(f: &Finding) -> String {
    let basis = format!("{}\u{0}{}\u{0}{}", f.file, f.title, f.evidence.join(","));
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(basis.as_bytes()))
}

/// Hidden marker embedded in each inline comment so a later run can tell
/// which findings it has already posted. Invisible in GitHub's rendered
/// Markdown.
fn comment_marker(f: &Finding) -> String {
    format!("<!-- nitpik:{} -->", fingerprint(f))
}

/// Extract the set of nitpik fingerprints already present in a batch of
/// existing comment bodies.
fn extract_markers<'a>(bodies: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    let mut out = HashSet::new();
    for body in bodies {
        let mut rest = body;
        while let Some(start) = rest.find("<!-- nitpik:") {
            let after = &rest[start + "<!-- nitpik:".len()..];
            if let Some(end) = after.find(" -->") {
                out.insert(after[..end].trim().to_string());
                rest = &after[end..];
            } else {
                break;
            }
        }
    }
    out
}

/// Split findings into those not yet posted (by fingerprint) and a count
/// of those skipped because an earlier run already commented on them.
fn partition_new(findings: &[Finding], existing: &HashSet<String>) -> (Vec<Finding>, usize) {
    let mut fresh = Vec::new();
    let mut skipped = 0usize;
    for f in findings {
        if existing.contains(&fingerprint(f)) {
            skipped += 1;
        } else {
            fresh.push(f.clone());
        }
    }
    (fresh, skipped)
}

/// Decide whether to post a review at all. Post when there's something new
/// to say, or on a first-ever clean pass (no findings and no prior nitpik
/// comments) — but stay quiet on re-runs where nothing is new, so the bot
/// doesn't re-spam an unchanged PR.
fn should_post(total_findings: usize, new_findings: usize, had_prior_comments: bool) -> bool {
    new_findings > 0 || (total_findings == 0 && !had_prior_comments)
}

/// Errors from the GitHub PR review API path.
#[derive(Error, Debug)]
pub enum GithubPrReviewError {
    #[error("missing environment variable: {0}")]
    MissingEnvVar(String),

    #[error("could not determine pull request number (set GITHUB_REF or GITHUB_EVENT_PATH)")]
    NoPullRequest,

    #[error("invalid GITHUB_REPOSITORY (expected 'owner/repo'): {0}")]
    InvalidRepository(String),

    #[error("API request failed: {0}")]
    ApiError(String),
}

/// GitHub PR review formatter — renders the API payload as JSON.
pub struct GithubPrReviewFormatter;

impl OutputFormatter for GithubPrReviewFormatter {
    fn format(&self, findings: &[Finding]) -> String {
        // No env here (pure) — the run-link footer and cross-run dedup are
        // applied by the publisher; the rendered payload comments on all.
        let payload = build_payload(findings, findings, "");
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Build the inline-comment array for the review.
///
/// Each finding anchors to the `RIGHT` (new-file) side. Multi-line
/// findings (`end_line` past `line`) use GitHub's `start_line`/`line`
/// range form.
fn comments_json(findings: &[Finding]) -> Vec<serde_json::Value> {
    findings
        .iter()
        .map(|f| {
            let (line, start_line) = match f.end_line {
                Some(end) if end > f.line => (end, Some(f.line)),
                _ => (f.line, None),
            };
            let mut comment = serde_json::json!({
                "path": f.file,
                "side": "RIGHT",
                "line": line,
                "body": format_comment_body(f),
            });
            if let Some(start) = start_line {
                comment["start_line"] = serde_json::json!(start);
                comment["start_side"] = serde_json::json!("RIGHT");
            }
            comment
        })
        .collect()
}

/// Format a single finding as a Markdown inline-comment body.
fn format_comment_body(f: &Finding) -> String {
    let mut body = format!(
        "{} **{}** ({})\n\n{}",
        f.severity.emoji(),
        f.title,
        f.severity,
        f.message
    );
    if let Some(ref suggestion) = f.suggestion {
        body.push_str(&format!("\n\n**Suggestion:** {suggestion}"));
    }
    body.push_str(&format!("\n\n_— agent: {}_", f.agent));
    body.push_str(&format!("\n\n{}", comment_marker(f)));
    body
}

/// Build the top-level review summary body. `footer` is appended verbatim
/// (the publisher passes a run link; the formatter passes "").
fn review_body(findings: &[Finding], footer: &str) -> String {
    let summary = crate::models::finding::Summary::from_findings(findings);
    format!(
        "**{}** found {} {} ({} error{}, {} warning{}, {} info)\n\n_{}_{}",
        crate::constants::APP_NAME,
        summary.total,
        if summary.total == 1 {
            "finding"
        } else {
            "findings"
        },
        summary.errors,
        if summary.errors == 1 { "" } else { "s" },
        summary.warnings,
        if summary.warnings == 1 { "" } else { "s" },
        summary.info,
        crate::constants::AI_DISCLOSURE,
        footer,
    )
}

/// Assemble the full `createReview` request body. The summary body counts
/// `summary_findings` (the PR's whole current state) while inline comments
/// are posted only for `comment_findings` (the not-yet-posted subset) —
/// the two differ when cross-run dedup suppresses already-posted findings.
/// `commit_id` is intentionally omitted so GitHub anchors to the PR's
/// latest commit — avoiding the merge-ref-vs-head-SHA ambiguity on
/// `pull_request` events.
fn build_payload(
    summary_findings: &[Finding],
    comment_findings: &[Finding],
    footer: &str,
) -> serde_json::Value {
    serde_json::json!({
        "event": "COMMENT",
        "body": review_body(summary_findings, footer),
        "comments": comments_json(comment_findings),
    })
}

/// Run-link footer pointing at the originating Actions run, when the
/// workflow env exposes one.
fn run_footer(env: &Env) -> String {
    match (
        env.var("GITHUB_SERVER_URL"),
        env.var("GITHUB_REPOSITORY"),
        env.var("GITHUB_RUN_ID"),
    ) {
        (Ok(server), Ok(repo), Ok(run_id)) => {
            format!("\n\n[View run]({server}/{repo}/actions/runs/{run_id})")
        }
        _ => String::new(),
    }
}

fn require_env(env: &Env, name: &str) -> Result<String, GithubPrReviewError> {
    env.var(name)
        .map_err(|_| GithubPrReviewError::MissingEnvVar(name.into()))
}

/// Resolve the PR number from `GITHUB_REF` (`refs/pull/<n>/merge`) or,
/// failing that, the `pull_request.number` / `number` field of the event
/// payload at `GITHUB_EVENT_PATH`.
fn resolve_pr_number(env: &Env) -> Result<u64, GithubPrReviewError> {
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
            .or_else(|| json.get("number"))
            .and_then(|n| n.as_u64());
        if let Some(n) = n {
            return Ok(n);
        }
    }

    Err(GithubPrReviewError::NoPullRequest)
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
        post_review(findings, self.env).await?;
        Ok(())
    }
}

/// Post the findings as a PR review via the GitHub REST API.
pub async fn post_review(findings: &[Finding], env: &Env) -> Result<(), GithubPrReviewError> {
    let token = require_env(env, "GITHUB_TOKEN")?;
    let repository = require_env(env, "GITHUB_REPOSITORY")?;
    let (owner, repo) = repository
        .split_once('/')
        .ok_or_else(|| GithubPrReviewError::InvalidRepository(repository.clone()))?;
    let pr_number = resolve_pr_number(env)?;

    let api_base = env
        .var("GITHUB_API_URL")
        .unwrap_or_else(|_| "https://api.github.com".to_string());
    let api_base = api_base.trim_end_matches('/');

    let client = crate::http::build_client()
        .map_err(|e| GithubPrReviewError::ApiError(format!("failed to build HTTP client: {e}")))?;

    // Cross-run dedup: skip findings the bot already commented on. Fail open
    // (treat all as new) if the probe errors — never block a review on it.
    let existing = fetch_existing_markers(&client, api_base, owner, repo, pr_number, &token).await;
    let had_prior_comments = !existing.is_empty();
    let (new_findings, skipped) = partition_new(findings, &existing);

    if !should_post(findings.len(), new_findings.len(), had_prior_comments) {
        return Ok(());
    }

    let mut footer = run_footer(env);
    if skipped > 0 {
        footer.push_str(&format!(
            "\n\n_({skipped} finding(s) already reported in earlier reviews.)_"
        ));
    }
    let payload = build_payload(findings, &new_findings, &footer);

    let url = format!("{api_base}/repos/{owner}/{repo}/pulls/{pr_number}/reviews");
    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(&payload)
        .send()
        .await
        .map_err(|e| GithubPrReviewError::ApiError(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<no body>".to_string());
        return Err(GithubPrReviewError::ApiError(format!(
            "review creation failed with HTTP {status}: {body}"
        )));
    }

    Ok(())
}

/// Fetch the fingerprint markers of nitpik's existing inline comments on
/// the PR, so a re-run doesn't repost them. Best-effort: any error (or a
/// non-success status) yields an empty set, so dedup degrades to "post
/// everything" rather than blocking the review.
async fn fetch_existing_markers(
    client: &reqwest::Client,
    api_base: &str,
    owner: &str,
    repo: &str,
    pr_number: u64,
    token: &str,
) -> HashSet<String> {
    const MAX_PAGES: u32 = 10;
    let mut bodies: Vec<String> = Vec::new();

    for page in 1..=MAX_PAGES {
        let url = format!(
            "{api_base}/repos/{owner}/{repo}/pulls/{pr_number}/comments?per_page=100&page={page}"
        );
        let resp = match client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => break,
        };
        let comments: Vec<serde_json::Value> = match resp.json().await {
            Ok(c) => c,
            Err(_) => break,
        };
        let page_len = comments.len();
        for c in comments {
            if let Some(body) = c.get("body").and_then(|b| b.as_str()) {
                bodies.push(body.to_string());
            }
        }
        if page_len < 100 {
            break;
        }
    }

    extract_markers(bodies.iter().map(|s| s.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

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
                agent: "backend".to_string(),
                evidence: Vec::new(),
            },
        ]
    }

    fn parse(findings: &[Finding]) -> serde_json::Value {
        serde_json::from_str(&GithubPrReviewFormatter.format(findings)).unwrap()
    }

    #[test]
    fn render_produces_comment_review() {
        let p = parse(&sample_findings());
        assert_eq!(p["event"], "COMMENT");
        assert!(p["body"].as_str().unwrap().contains("2 findings"));
        assert_eq!(p["comments"].as_array().unwrap().len(), 2);
        // commit_id is intentionally omitted (GitHub defaults to latest).
        assert!(p.get("commit_id").is_none());
    }

    #[test]
    fn single_line_comment_uses_line_and_right_side() {
        let p = parse(&sample_findings());
        let c = &p["comments"][0];
        assert_eq!(c["path"], "src/main.rs");
        assert_eq!(c["line"], 10);
        assert_eq!(c["side"], "RIGHT");
        // No start_line for a single-line finding.
        assert!(c.get("start_line").is_none());
    }

    #[test]
    fn multi_line_comment_uses_start_line_range() {
        let p = parse(&sample_findings());
        let c = &p["comments"][1];
        // end_line (24) becomes `line`; original line (20) becomes start_line.
        assert_eq!(c["line"], 24);
        assert_eq!(c["start_line"], 20);
        assert_eq!(c["start_side"], "RIGHT");
        assert_eq!(c["side"], "RIGHT");
    }

    #[test]
    fn comment_body_has_emoji_severity_suggestion_and_agent() {
        let p = parse(&sample_findings());
        let body = p["comments"][0]["body"].as_str().unwrap();
        assert!(body.contains("🔴")); // error emoji
        assert!(body.contains("Bug"));
        assert!(body.contains("**Suggestion:** Fix the bug"));
        assert!(body.contains("agent: backend"));
        // Finding without a suggestion omits the section.
        let warn = p["comments"][1]["body"].as_str().unwrap();
        assert!(!warn.contains("Suggestion"));
    }

    #[test]
    fn empty_findings_still_posts_a_summary() {
        let p = parse(&[]);
        assert_eq!(p["event"], "COMMENT");
        assert!(p["body"].as_str().unwrap().contains("0 findings"));
        assert!(p["comments"].as_array().unwrap().is_empty());
    }

    #[test]
    fn single_finding_uses_singular() {
        let one = vec![sample_findings().remove(0)];
        let p = parse(&one);
        let body = p["body"].as_str().unwrap();
        assert!(body.contains("1 finding"));
        assert!(!body.contains("1 findings"));
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
    fn resolve_pr_number_missing_errors() {
        let env = Env::mock([("GITHUB_REF", "refs/heads/main")]);
        assert!(matches!(
            resolve_pr_number(&env),
            Err(GithubPrReviewError::NoPullRequest)
        ));
    }

    #[tokio::test]
    async fn post_missing_env_vars_cascade() {
        // Missing GITHUB_TOKEN
        let env = Env::mock(Vec::<(&str, &str)>::new());
        let err = post_review(&sample_findings(), &env).await.unwrap_err();
        assert!(err.to_string().contains("GITHUB_TOKEN"), "got: {err}");

        // Missing GITHUB_REPOSITORY
        let env = Env::mock([("GITHUB_TOKEN", "tok")]);
        let err = post_review(&sample_findings(), &env).await.unwrap_err();
        assert!(err.to_string().contains("GITHUB_REPOSITORY"), "got: {err}");

        // Bad repository shape
        let env = Env::mock([("GITHUB_TOKEN", "tok"), ("GITHUB_REPOSITORY", "no-slash")]);
        let err = post_review(&sample_findings(), &env).await.unwrap_err();
        assert!(
            err.to_string().contains("invalid GITHUB_REPOSITORY"),
            "got: {err}"
        );

        // Repo present but no PR number resolvable
        let env = Env::mock([
            ("GITHUB_TOKEN", "tok"),
            ("GITHUB_REPOSITORY", "owner/repo"),
            ("GITHUB_REF", "refs/heads/main"),
        ]);
        let err = post_review(&sample_findings(), &env).await.unwrap_err();
        assert!(
            err.to_string().contains("pull request number"),
            "got: {err}"
        );
    }

    #[test]
    fn run_footer_present_when_env_set() {
        let env = Env::mock([
            ("GITHUB_SERVER_URL", "https://github.com"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_RUN_ID", "42"),
        ]);
        let footer = run_footer(&env);
        assert!(footer.contains("https://github.com/o/r/actions/runs/42"));
    }

    // --- cross-run dedup (gap b) ---

    #[test]
    fn fingerprint_is_stable_across_line_shifts() {
        let mut a = sample_findings().remove(0);
        let mut b = a.clone();
        // Same file/title/evidence but different line (a later push shifted it).
        a.line = 10;
        b.line = 57;
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn fingerprint_differs_by_file_or_title() {
        let base = sample_findings().remove(0);
        let mut other_file = base.clone();
        other_file.file = "src/other.rs".into();
        let mut other_title = base.clone();
        other_title.title = "Different".into();
        assert_ne!(fingerprint(&base), fingerprint(&other_file));
        assert_ne!(fingerprint(&base), fingerprint(&other_title));
    }

    #[test]
    fn comment_body_embeds_marker() {
        let f = sample_findings().remove(0);
        let body = format_comment_body(&f);
        assert!(body.contains(&comment_marker(&f)));
        assert!(body.contains("<!-- nitpik:"));
    }

    #[test]
    fn extract_markers_finds_all() {
        let f = sample_findings();
        let bodies = [format_comment_body(&f[0]), "no marker here".to_string()];
        let markers = extract_markers(bodies.iter().map(|s| s.as_str()));
        assert!(markers.contains(&fingerprint(&f[0])));
        assert_eq!(markers.len(), 1);
    }

    #[test]
    fn partition_new_skips_already_posted() {
        let findings = sample_findings();
        // Pretend the first finding was already posted in an earlier review.
        let existing: HashSet<String> = [fingerprint(&findings[0])].into_iter().collect();
        let (fresh, skipped) = partition_new(&findings, &existing);
        assert_eq!(skipped, 1);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].title, findings[1].title);
    }

    #[test]
    fn should_post_policy() {
        // Something new to say → post.
        assert!(should_post(3, 2, true));
        // First-ever clean pass (no findings, no prior comments) → post once.
        assert!(should_post(0, 0, false));
        // Clean re-run after prior comments → stay quiet.
        assert!(!should_post(0, 0, true));
        // Findings exist but all already posted → stay quiet.
        assert!(!should_post(3, 0, true));
    }
}
