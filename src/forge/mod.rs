//! Forge abstraction — read + write access to a code-review platform's
//! pull/merge request.
//!
//! # Bounded Context: Forge Integration
//!
//! PR-native review isn't output *formatting* — it's a **forge
//! capability** with read and write halves that belong to the same
//! provider:
//!
//! - **Read** — fetch the PR/MR (title/body/head-sha) for author-intent
//!   context; fetch prior review comments for cross-run dedup.
//! - **Write** — post a review with inline comments anchored to the diff.
//!
//! Both halves are expressed against the forge-agnostic types in this
//! module ([`PullRequest`], [`ReviewDraft`], [`InlineComment`], …). Each
//! concrete forge ([`github::GithubForge`], [`gitlab::GitlabForge`])
//! translates these to/from its own API. The engine only ever sees the
//! neutral types.
//!
//! The shared, provider-neutral pieces live here: the cross-run dedup
//! helpers (fingerprint / markers / partition) and the [`ReviewDraft`]
//! builder (summary body + inline-comment bodies). The per-forge adapters
//! own only endpoints, auth, and — the real divergence — inline-comment
//! **anchoring**.
//!
//! ## Support status
//!
//! GitHub is the supported, advertised target. The GitLab adapter is
//! implemented against the same internal trait so the abstraction is
//! exercised by two real backends from the start, but it is **not yet
//! advertised** pending live testing — see `plans/pr-native-review/`.

pub mod bitbucket;
pub mod forgejo;
pub mod github;
pub mod gitlab;

use std::collections::HashSet;

use thiserror::Error;

use crate::env::Env;
use crate::models::finding::{Finding, Severity};

// ── Neutral data model ──────────────────────────────────────────────

/// A pull/merge request under review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub head_sha: String,
    pub base_ref: String,
    pub author: Option<String>,
}

/// An existing review comment already on the PR (used to recognize
/// findings a prior run already posted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingComment {
    pub body: String,
    pub author: Option<String>,
}

/// Which side of the diff a comment anchors to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The new (post-change) file — where nitpik findings live.
    Right,
    /// The old (pre-change) file.
    Left,
}

/// The review action to take when posting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEvent {
    /// Comment without an explicit approval state (what nitpik posts).
    Comment,
    Approve,
    RequestChanges,
}

/// One inline comment, anchored to a line on one side of the diff.
///
/// Deliberately minimal: `path` + `line` (+ optional `end_line` for a
/// range) + `side`. Each adapter does its own positioning from this —
/// GitHub wants `line`+`side`, GitLab needs a full `position` with diff
/// SHAs, etc. The neutral shape stays small on purpose so a forge can map
/// it without losing information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineComment {
    pub path: String,
    pub line: u32,
    pub end_line: Option<u32>,
    pub side: Side,
    pub body: String,
}

/// A complete review ready to post: a summary plus inline comments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDraft {
    pub summary: String,
    pub event: ReviewEvent,
    pub comments: Vec<InlineComment>,
}

/// An open review thread on the PR — used to retire prior feedback that a
/// later push has addressed (reply + resolve).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThread {
    /// Opaque forge thread identifier (e.g. GitHub's GraphQL node id),
    /// used to resolve the thread.
    pub id: String,
    /// Root comment id for posting a reply (GitHub's REST `databaseId`).
    pub root_comment_id: Option<u64>,
    /// nitpik fingerprint parsed from the root comment's hidden marker, if
    /// present — i.e. whether this thread is one nitpik authored.
    pub fingerprint: Option<String>,
    pub path: String,
    pub line: Option<u32>,
    /// The forge flags the anchored line as changed since the comment was
    /// written — a hint (not a verdict) that the finding may be addressed.
    pub outdated: bool,
    /// Root comment body — the prior finding text the judge evaluates.
    pub body: String,
}

/// Errors raised by a forge adapter.
#[derive(Error, Debug)]
pub enum ForgeError {
    #[error("missing environment variable: {0}")]
    MissingEnvVar(String),

    #[error("could not determine pull request number")]
    NoPullRequest,

    #[error("invalid repository identifier: {0}")]
    InvalidRepository(String),

    #[error("API request failed: {0}")]
    ApiError(String),
}

// ── Forge trait ─────────────────────────────────────────────────────

/// Read + write access to a single PR/MR on a code-review platform.
#[async_trait::async_trait]
pub trait Forge: Send + Sync {
    /// The PR/MR under review: number, title, body, head/base, author.
    async fn pull_request(&self) -> Result<PullRequest, ForgeError>;

    /// Existing review comments on the PR (for cross-run dedup markers and
    /// prior-thread context). Best-effort: callers fail open on `Err`.
    async fn existing_review_comments(&self) -> Result<Vec<ExistingComment>, ForgeError>;

    /// Post a review (summary + inline comments) on the PR.
    async fn post_review(&self, draft: &ReviewDraft) -> Result<(), ForgeError>;

    /// Open (unresolved) review threads on the PR. Used to retire prior
    /// feedback the latest push has addressed. Default: none — a forge
    /// without thread-resolution support simply has nothing to retire.
    async fn open_review_threads(&self) -> Result<Vec<ReviewThread>, ForgeError> {
        Ok(Vec::new())
    }

    /// Reply to a thread's root comment and mark the thread resolved.
    /// Only ever called with a thread returned by [`Self::open_review_threads`],
    /// so the default (unsupported) is unreachable for forges that return no
    /// threads.
    async fn reply_and_resolve(
        &self,
        _thread: &ReviewThread,
        _reply: &str,
    ) -> Result<(), ForgeError> {
        Err(ForgeError::ApiError(
            "thread resolution is not supported for this forge".into(),
        ))
    }

    /// A run-link footer appended to the review summary, when the CI
    /// environment exposes a link. Default: none.
    fn run_footer(&self) -> String {
        String::new()
    }
}

/// Detect the active forge from the environment, returning an adapter or
/// `None` (no forge → caller behaves like the annotation-only path).
///
/// GitHub is preferred when its token is present; GitLab, then Forgejo,
/// are tried next. Each adapter keys on distinct env vars, so the order
/// only matters if two forges' variables are present at once. Only GitHub
/// is advertised today — the GitLab and Forgejo branches keep the
/// abstraction honest (real backends) but are gated on live testing.
pub fn detect(env: &Env) -> Option<Box<dyn Forge>> {
    if let Ok(f) = github::GithubForge::from_env(env) {
        return Some(Box::new(f));
    }
    if let Ok(f) = gitlab::GitlabForge::from_env(env) {
        return Some(Box::new(f));
    }
    if let Ok(f) = forgejo::ForgejoForge::from_env(env) {
        return Some(Box::new(f));
    }
    if let Ok(f) = bitbucket::BitbucketForge::from_env(env) {
        return Some(Box::new(f));
    }
    None
}

// ── Shared review orchestration (forge-agnostic) ────────────────────

/// Read existing comments, suppress findings already posted, and post a
/// review — staying quiet on a re-run with nothing new to say.
///
/// `event` is the review action (see [`review_event_for`]). Returns
/// `Ok(true)` if a review was posted, `Ok(false)` if posting was skipped
/// (nothing new). Reading existing comments fails open: a probe error
/// degrades to "post everything" rather than blocking the review.
///
/// `force` bypasses the quiet-on-re-run gate ([`should_post`]) so an
/// explicit re-review (e.g. `@nitpik review`) always posts a summary even
/// when nothing changed. Per-finding dedup still applies — inline comments
/// are only added for findings not already posted — so a forced re-review
/// doesn't duplicate existing comment threads.
pub async fn publish_review(
    forge: &dyn Forge,
    findings: &[Finding],
    event: ReviewEvent,
    force: bool,
) -> Result<bool, ForgeError> {
    let existing = forge.existing_review_comments().await.unwrap_or_default();
    let markers = extract_markers(existing.iter().map(|c| c.body.as_str()));
    let had_prior = !markers.is_empty();
    let (fresh, skipped) = partition_new(findings, &markers);

    if !force && !should_post(findings.len(), fresh.len(), had_prior) {
        return Ok(false);
    }

    let mut footer = forge.run_footer();
    if skipped > 0 {
        footer.push_str(&format!(
            "\n\n_({skipped} finding(s) already reported in earlier reviews.)_"
        ));
    }
    let draft = build_review_draft(findings, &fresh, &footer, event);
    forge.post_review(&draft).await?;
    Ok(true)
}

/// Choose the review action from findings and an optional request-changes
/// threshold. With a threshold set, the review is [`ReviewEvent::RequestChanges`]
/// when any finding is at or above it; otherwise — and by default (no
/// threshold) — [`ReviewEvent::Comment`]. nitpik never auto-approves, so a
/// human still owns the approval state.
pub fn review_event_for(findings: &[Finding], request_changes: Option<Severity>) -> ReviewEvent {
    match request_changes {
        Some(threshold) if findings.iter().any(|f| f.severity >= threshold) => {
            ReviewEvent::RequestChanges
        }
        _ => ReviewEvent::Comment,
    }
}

// ── ReviewDraft construction (forge-agnostic) ───────────────────────

/// Build a [`ReviewDraft`]. The summary counts `summary_findings` (the
/// PR's whole current state) while inline comments are produced only for
/// `comment_findings` (the not-yet-posted subset) — the two differ when
/// cross-run dedup suppresses already-posted findings. `footer` is
/// appended verbatim to the summary body.
pub fn build_review_draft(
    summary_findings: &[Finding],
    comment_findings: &[Finding],
    footer: &str,
    event: ReviewEvent,
) -> ReviewDraft {
    ReviewDraft {
        summary: review_body(summary_findings, footer),
        event,
        comments: comment_findings.iter().map(inline_comment).collect(),
    }
}

/// Map one finding to a neutral inline comment on the new-file side.
fn inline_comment(f: &Finding) -> InlineComment {
    InlineComment {
        path: f.file.clone(),
        line: f.line,
        end_line: f.end_line.filter(|&e| e > f.line),
        side: Side::Right,
        body: format_comment_body(f),
    }
}

/// Format a single finding as a Markdown inline-comment body, ending with
/// the hidden dedup marker.
pub fn format_comment_body(f: &Finding) -> String {
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

/// Build the top-level review summary body. `footer` is appended verbatim.
pub fn review_body(findings: &[Finding], footer: &str) -> String {
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

// ── Cross-run dedup (forge-agnostic) ────────────────────────────────

/// Stable, line-independent fingerprint of a finding, used to recognize
/// the same issue across PR pushes (line numbers shift; `path | title |
/// evidence` does not). Hashed so it embeds cleanly in a comment marker.
pub fn fingerprint(f: &Finding) -> String {
    let basis = format!("{}\u{0}{}\u{0}{}", f.file, f.title, f.evidence.join(","));
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(basis.as_bytes()))
}

/// Hidden marker embedded in each inline comment so a later run can tell
/// which findings it has already posted. Invisible in rendered Markdown.
pub fn comment_marker(f: &Finding) -> String {
    format!("<!-- nitpik:{} -->", fingerprint(f))
}

/// Extract the set of nitpik fingerprints present in a batch of comment
/// bodies.
pub fn extract_markers<'a>(bodies: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
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
pub fn partition_new(findings: &[Finding], existing: &HashSet<String>) -> (Vec<Finding>, usize) {
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
pub fn should_post(total_findings: usize, new_findings: usize, had_prior_comments: bool) -> bool {
    new_findings > 0 || (total_findings == 0 && !had_prior_comments)
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

    #[test]
    fn draft_summary_counts_all_inline_comments_only_fresh() {
        let all = sample_findings();
        let fresh = &all[1..]; // pretend the first was already posted
        let draft = build_review_draft(&all, fresh, "", ReviewEvent::Comment);
        assert_eq!(draft.event, ReviewEvent::Comment);
        assert!(draft.summary.contains("2 findings"));
        assert_eq!(draft.comments.len(), 1);
        assert_eq!(draft.comments[0].path, "src/lib.rs");
    }

    #[test]
    fn draft_carries_the_chosen_event() {
        let all = sample_findings();
        let draft = build_review_draft(&all, &all, "", ReviewEvent::RequestChanges);
        assert_eq!(draft.event, ReviewEvent::RequestChanges);
    }

    #[test]
    fn review_event_defaults_to_comment_without_threshold() {
        let f = sample_findings(); // contains an error + a warning
        assert_eq!(review_event_for(&f, None), ReviewEvent::Comment);
    }

    #[test]
    fn review_event_requests_changes_when_threshold_met() {
        let f = sample_findings(); // has an Error finding
        assert_eq!(
            review_event_for(&f, Some(Severity::Error)),
            ReviewEvent::RequestChanges
        );
        assert_eq!(
            review_event_for(&f, Some(Severity::Warning)),
            ReviewEvent::RequestChanges
        );
    }

    #[test]
    fn review_event_comments_when_threshold_not_met() {
        // Only an info finding, threshold error → stays a comment.
        let f = vec![Finding {
            severity: Severity::Info,
            ..sample_findings().remove(0)
        }];
        assert_eq!(
            review_event_for(&f, Some(Severity::Error)),
            ReviewEvent::Comment
        );
        // No findings at all → comment (never request changes on a clean PR).
        assert_eq!(review_event_for(&[], Some(Severity::Error)), ReviewEvent::Comment);
    }

    #[test]
    fn inline_comment_carries_range_and_marker() {
        let f = &sample_findings()[1];
        let c = inline_comment(f);
        assert_eq!(c.line, 20);
        assert_eq!(c.end_line, Some(24));
        assert_eq!(c.side, Side::Right);
        assert!(c.body.contains(&comment_marker(f)));
    }

    #[test]
    fn single_line_finding_has_no_range() {
        let c = inline_comment(&sample_findings()[0]);
        assert_eq!(c.line, 10);
        assert_eq!(c.end_line, None);
    }

    #[test]
    fn comment_body_has_emoji_severity_suggestion_and_agent() {
        let f = sample_findings();
        let body = format_comment_body(&f[0]);
        assert!(body.contains("🔴"));
        assert!(body.contains("Bug"));
        assert!(body.contains("**Suggestion:** Fix the bug"));
        assert!(body.contains("agent: correctness"));
        assert!(body.contains("<!-- nitpik:"));
        // A finding without a suggestion omits that section.
        assert!(!format_comment_body(&f[1]).contains("Suggestion"));
    }

    #[test]
    fn fingerprint_is_stable_across_line_shifts() {
        let mut a = sample_findings().remove(0);
        let mut b = a.clone();
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
        let existing: HashSet<String> = [fingerprint(&findings[0])].into_iter().collect();
        let (fresh, skipped) = partition_new(&findings, &existing);
        assert_eq!(skipped, 1);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].title, findings[1].title);
    }

    #[test]
    fn should_post_policy() {
        assert!(should_post(3, 2, true));
        assert!(should_post(0, 0, false));
        assert!(!should_post(0, 0, true));
        assert!(!should_post(3, 0, true));
    }

    /// A mock forge that reports its PR as already carrying every sample
    /// finding (so the quiet-on-re-run gate would normally suppress), and
    /// records the draft it was asked to post.
    struct RecordingForge {
        prior: Vec<ExistingComment>,
        posted: std::sync::Mutex<Option<ReviewDraft>>,
    }

    #[async_trait::async_trait]
    impl Forge for RecordingForge {
        async fn pull_request(&self) -> Result<PullRequest, ForgeError> {
            Err(ForgeError::NoPullRequest)
        }
        async fn existing_review_comments(&self) -> Result<Vec<ExistingComment>, ForgeError> {
            Ok(self.prior.clone())
        }
        async fn post_review(&self, draft: &ReviewDraft) -> Result<(), ForgeError> {
            *self.posted.lock().unwrap() = Some(draft.clone());
            Ok(())
        }
    }

    /// Build a forge whose PR already has both sample findings posted.
    fn forge_with_all_posted() -> RecordingForge {
        let prior = sample_findings()
            .iter()
            .map(|f| ExistingComment {
                body: format_comment_body(f),
                author: Some("nitpik".into()),
            })
            .collect();
        RecordingForge {
            prior,
            posted: std::sync::Mutex::new(None),
        }
    }

    #[tokio::test]
    async fn publish_skips_re_run_with_nothing_new() {
        let forge = forge_with_all_posted();
        let posted =
            publish_review(&forge, &sample_findings(), ReviewEvent::Comment, false)
                .await
                .unwrap();
        assert!(!posted, "should stay quiet when nothing is new");
        assert!(forge.posted.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn force_posts_even_when_nothing_new() {
        let forge = forge_with_all_posted();
        let posted = publish_review(&forge, &sample_findings(), ReviewEvent::Comment, true)
            .await
            .unwrap();
        assert!(posted, "force must post a review on re-run");
        let draft = forge.posted.lock().unwrap().clone().unwrap();
        // Per-finding dedup still applies: both findings are already posted,
        // so the forced review carries a summary but no duplicate threads.
        assert!(draft.comments.is_empty());
        assert!(draft.summary.contains("2 findings"));
    }

    #[test]
    fn detect_prefers_github_when_token_present() {
        let env = Env::mock([
            ("GITHUB_TOKEN", "t"),
            ("GITHUB_REPOSITORY", "o/r"),
            ("GITHUB_REF", "refs/pull/3/merge"),
            // GitLab vars also present — GitHub still wins.
            ("CI_API_V4_URL", "https://gitlab.com/api/v4"),
            ("CI_PROJECT_ID", "1"),
            ("CI_MERGE_REQUEST_IID", "2"),
            ("GITLAB_TOKEN", "g"),
        ]);
        assert!(detect(&env).is_some());
    }

    #[test]
    fn detect_falls_through_to_gitlab() {
        let env = Env::mock([
            ("CI_API_V4_URL", "https://gitlab.com/api/v4"),
            ("CI_PROJECT_ID", "1"),
            ("CI_MERGE_REQUEST_IID", "2"),
            ("GITLAB_TOKEN", "g"),
        ]);
        assert!(detect(&env).is_some());
    }

    #[test]
    fn detect_falls_through_to_forgejo() {
        let env = Env::mock([
            ("CI_FORGE_URL", "https://codeberg.org"),
            ("CI_REPO_OWNER", "o"),
            ("CI_REPO_NAME", "r"),
            ("CI_COMMIT_PULL_REQUEST", "7"),
            ("CI_COMMIT_SHA", "abc123"),
            ("FORGEJO_TOKEN", "tok"),
        ]);
        assert!(detect(&env).is_some());
    }

    #[test]
    fn detect_none_without_a_forge() {
        let env = Env::mock([("SOMETHING", "else")]);
        assert!(detect(&env).is_none());
    }
}
