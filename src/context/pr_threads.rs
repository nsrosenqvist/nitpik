//! Prior PR review-thread context.
//!
//! Fetches the review comments already on the pull request — nitpik's own
//! earlier findings plus any human replies — so reviewers can avoid
//! re-raising points that were addressed or explicitly accepted, and can
//! weigh the author's responses. This is the read half of the
//! [`crate::forge`] abstraction: the CI event payload carries PR metadata
//! but not existing comments, so it must come from the forge API.
//!
//! Opt-in (`--pr-threads`) and best-effort: any forge/API error yields
//! `None` rather than blocking the review. The comments are untrusted,
//! human-authored text; the prompt frames them as context to weigh, never
//! as instructions (see
//! [`build_system_addendum`](crate::orchestrator::prompt::build_system_addendum)).

use crate::env::Env;
use crate::forge::{self, ExistingComment};

/// Max comments rendered into the context block. A long-running PR can
/// accumulate dozens of threads; past this it stops being useful context
/// and only inflates every reviewer's prompt.
const MAX_COMMENTS: usize = 30;

/// Per-comment character cap — keep each line a tight gist, not a transcript.
const MAX_COMMENT_CHARS: usize = 280;

/// Overall cap on the assembled block.
const MAX_BLOCK_CHARS: usize = 4000;

/// Fetch and format prior review threads, or `None` when
/// unavailable/disabled.
///
/// `enabled` is the caller's opt-in (`--pr-threads`); when false this
/// returns `None` without touching the environment or network.
pub async fn gather_prior_threads(env: &Env, enabled: bool) -> Option<String> {
    if !enabled {
        return None;
    }
    let forge = forge::detect(env)?;
    let comments = forge.existing_review_comments().await.ok()?;
    format_threads(&comments)
}

/// Render existing comments into a labeled, capped context block. nitpik's
/// own comments (recognized by the hidden dedup marker) are labeled
/// `[nitpik]`; everything else is labeled by author. Returns `None` when
/// there is nothing to show.
fn format_threads(comments: &[ExistingComment]) -> Option<String> {
    let mut out = String::new();
    let mut shown = 0usize;

    for c in comments {
        if shown >= MAX_COMMENTS {
            break;
        }
        let is_nitpik = c.body.contains("<!-- nitpik:");
        let gist = gist(&strip_markers(&c.body));
        if gist.is_empty() {
            continue;
        }
        let label = if is_nitpik {
            "nitpik".to_string()
        } else {
            c.author.clone().unwrap_or_else(|| "unknown".to_string())
        };
        let line = format!("- [{label}] {gist}\n");

        // Stop before exceeding the overall cap, but always keep ≥1 line.
        if !out.is_empty() && out.len() + line.len() > MAX_BLOCK_CHARS {
            out.push_str("- […earlier comments omitted]\n");
            break;
        }
        out.push_str(&line);
        shown += 1;
    }

    if out.is_empty() { None } else { Some(out) }
}

/// Remove the hidden `<!-- nitpik:… -->` dedup markers from a body.
fn strip_markers(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<!-- nitpik:") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find(" -->") {
            Some(end) => rest = &after[end + " -->".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Collapse a comment body to a single-line gist: whitespace flattened,
/// truncated to [`MAX_COMMENT_CHARS`] on a char boundary.
fn gist(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_COMMENT_CHARS {
        return flat;
    }
    let truncated: String = flat.chars().take(MAX_COMMENT_CHARS).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(body: &str, author: Option<&str>) -> ExistingComment {
        ExistingComment {
            body: body.to_string(),
            author: author.map(String::from),
        }
    }

    #[test]
    fn empty_yields_none() {
        assert!(format_threads(&[]).is_none());
    }

    #[test]
    fn labels_nitpik_and_human_comments() {
        let comments = vec![
            comment(
                "🔴 **SQL injection** (error)\n\nUse a parameterized query.\n\n<!-- nitpik:abc123 -->",
                Some("nitpik-bot"),
            ),
            comment(
                "Won't fix — user_id is an int from the router.",
                Some("alice"),
            ),
        ];
        let out = format_threads(&comments).unwrap();
        assert!(out.contains("[nitpik] 🔴 **SQL injection** (error) Use a parameterized query."));
        assert!(out.contains("[alice] Won't fix — user_id is an int from the router."));
        // The hidden marker is stripped from the rendered context.
        assert!(!out.contains("<!-- nitpik:"));
    }

    #[test]
    fn unknown_author_falls_back() {
        let out = format_threads(&[comment("Looks good to me.", None)]).unwrap();
        assert!(out.contains("[unknown] Looks good to me."));
    }

    #[test]
    fn skips_comments_that_are_only_a_marker() {
        // A comment whose entire body is a marker reduces to empty → skipped.
        assert!(format_threads(&[comment("<!-- nitpik:deadbeef -->", Some("b"))]).is_none());
    }

    #[test]
    fn caps_number_of_comments() {
        let many: Vec<ExistingComment> = (0..50)
            .map(|i| comment(&format!("comment {i}"), Some("u")))
            .collect();
        let out = format_threads(&many).unwrap();
        assert_eq!(out.lines().count(), MAX_COMMENTS);
    }

    #[test]
    fn long_comment_is_truncated() {
        let long = "x ".repeat(400); // ~800 chars, well over the per-comment cap
        let out = format_threads(&[comment(&long, Some("u"))]).unwrap();
        assert!(out.contains('…'));
        assert!(out.len() < MAX_COMMENT_CHARS + 50);
    }

    #[test]
    fn strip_markers_removes_all_occurrences() {
        let s = strip_markers("a <!-- nitpik:1 --> b <!-- nitpik:2 --> c");
        assert_eq!(s.split_whitespace().collect::<Vec<_>>().join(" "), "a b c");
    }
}
