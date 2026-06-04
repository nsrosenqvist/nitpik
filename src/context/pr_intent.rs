//! PR author-intent detection.
//!
//! Surfaces the pull request's title and description (what the author says
//! the change is *for*) so reviewers can judge the diff against its stated
//! purpose — catching "the code does X but the PR claims Y" mismatches and
//! suppressing findings that the description already explains as intentional.
//!
//! Two sources, in priority order:
//! 1. [`ENV_PR_INTENT`](crate::constants::ENV_PR_INTENT) — an explicit
//!    override, for non-GitHub forges or local runs.
//! 2. The GitHub Actions event payload at `GITHUB_EVENT_PATH`
//!    (`pull_request.title` + `pull_request.body`).
//!
//! The intent is untrusted author-supplied text; the prompt frames it as a
//! description to verify, not as instructions (see
//! [`build_system_addendum`](crate::orchestrator::prompt::build_system_addendum)).

use crate::constants::ENV_PR_INTENT;
use crate::env::Env;

/// Cap on the assembled intent length. A PR body can be arbitrarily long
/// (templates, checklists, logs); past this it stops being useful context
/// and only inflates every reviewer's prompt.
const MAX_INTENT_CHARS: usize = 4000;

/// Resolve the PR author's stated intent, or `None` when unavailable/disabled.
///
/// `enabled` is the caller's opt-out (e.g. `--no-pr-intent`); when false this
/// returns `None` without touching the environment.
pub fn detect_pr_intent(env: &Env, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }

    if let Ok(raw) = env.var(ENV_PR_INTENT) {
        return clean(&raw);
    }

    intent_from_github_event(env)
}

/// Read `pull_request.title` / `pull_request.body` from the GitHub Actions
/// event payload. Returns `None` if not running on a `pull_request` event or
/// the payload is missing/unparseable.
fn intent_from_github_event(env: &Env) -> Option<String> {
    let event_path = env.var("GITHUB_EVENT_PATH").ok()?;
    let raw = std::fs::read_to_string(&event_path).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let pr = json.get("pull_request")?;

    let title = pr.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let body = pr.get("body").and_then(|v| v.as_str()).unwrap_or("");

    format_intent(title, body)
}

/// Combine a title and body into a single labeled block, trimming and
/// truncating. Returns `None` when both are effectively empty.
fn format_intent(title: &str, body: &str) -> Option<String> {
    let title = title.trim();
    let body = body.trim();

    let mut out = String::new();
    if !title.is_empty() {
        out.push_str("Title: ");
        out.push_str(title);
    }
    if !body.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(body);
    }

    clean(&out)
}

/// Trim, truncate to [`MAX_INTENT_CHARS`], and drop if empty.
fn clean(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.len() <= MAX_INTENT_CHARS {
        return Some(s.to_string());
    }
    // Truncate on a char boundary and mark the cut.
    let mut end = MAX_INTENT_CHARS;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}\n\n[…description truncated]", &s[..end]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_returns_none() {
        let env = Env::mock([(ENV_PR_INTENT, "Title: x")]);
        assert!(detect_pr_intent(&env, false).is_none());
    }

    #[test]
    fn explicit_override_wins() {
        let env = Env::mock([(ENV_PR_INTENT, "  Fix the billing race  ")]);
        assert_eq!(
            detect_pr_intent(&env, true).as_deref(),
            Some("Fix the billing race")
        );
    }

    #[test]
    fn blank_override_yields_none() {
        let env = Env::mock([(ENV_PR_INTENT, "   \n  ")]);
        assert!(detect_pr_intent(&env, true).is_none());
    }

    #[test]
    fn reads_github_event_title_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        std::fs::write(
            &path,
            r#"{"pull_request":{"title":"Add retry logic","body":"Retries billing on 5xx."}}"#,
        )
        .unwrap();
        let env = Env::mock([("GITHUB_EVENT_PATH", path.to_str().unwrap())]);

        let intent = detect_pr_intent(&env, true).unwrap();
        assert!(intent.contains("Title: Add retry logic"));
        assert!(intent.contains("Retries billing on 5xx."));
    }

    #[test]
    fn github_event_title_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        std::fs::write(
            &path,
            r#"{"pull_request":{"title":"Bump deps","body":null}}"#,
        )
        .unwrap();
        let env = Env::mock([("GITHUB_EVENT_PATH", path.to_str().unwrap())]);

        assert_eq!(
            detect_pr_intent(&env, true).as_deref(),
            Some("Title: Bump deps")
        );
    }

    #[test]
    fn non_pr_event_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("event.json");
        std::fs::write(&path, r#"{"ref":"refs/heads/main"}"#).unwrap();
        let env = Env::mock([("GITHUB_EVENT_PATH", path.to_str().unwrap())]);

        assert!(detect_pr_intent(&env, true).is_none());
    }

    #[test]
    fn no_sources_yields_none() {
        let env = Env::mock(Vec::<(&str, &str)>::new());
        assert!(detect_pr_intent(&env, true).is_none());
    }

    #[test]
    fn long_body_is_truncated() {
        let body = "x".repeat(MAX_INTENT_CHARS + 500);
        let intent = format_intent("T", &body).unwrap();
        assert!(intent.contains("[…description truncated]"));
        assert!(intent.len() < MAX_INTENT_CHARS + 100);
    }
}
