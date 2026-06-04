//! Rolling pull-request summary (Phase C, gap c).
//!
//! Produces a durable, cross-run "what this PR does / subsystems touched /
//! open risks" note that is fed into every reviewer's baseline context.
//! Unlike per-file prior findings, the summary gives each reviewer the
//! whole-PR picture. It is regenerated each run from the current diff plus
//! the prior summary (so it accumulates context across pushes) and
//! persisted per review scope (branch) in the cache store.
//!
//! Generation is gated (off by default) because it adds one LLM call per
//! run. The prompt-building and digest here are pure and unit-tested; the
//! actual call lives behind [`crate::providers::ReviewProvider::summarize`].

use crate::models::diff::{DiffLineType, FileDiff};

/// Maximum characters of diff body included in the summary prompt. Keeps
/// the extra call cheap and bounded on large PRs; the file list is always
/// included in full so the model still sees the breadth of the change.
const MAX_DIFF_CHARS: usize = 6000;

/// Maximum files listed individually before truncating with a count.
const MAX_FILES: usize = 200;

/// System prompt for the rolling-summary call.
pub fn summary_system_prompt() -> String {
    "You summarize a pull request for code reviewers. Produce a concise, \
     factual summary in this shape:\n\n\
     - One sentence on what the change accomplishes.\n\
     - A short bullet list of the subsystems/areas touched.\n\
     - A short bullet list of open risks or things reviewers should watch \
     (omit if none are evident).\n\n\
     Describe only what the diff shows — do not speculate about intent you \
     cannot see, invent requirements, or evaluate code quality (that is the \
     reviewers' job). Keep it under ~150 words. If a previous summary is \
     provided, refresh it to reflect the latest changes rather than \
     restating it verbatim."
        .to_string()
}

/// Build the user prompt: the prior summary (if any) plus a compact digest
/// of the current diff.
pub fn build_summary_user_prompt(diffs: &[FileDiff<'_>], prior: Option<&str>) -> String {
    let mut s = String::new();
    if let Some(prior) = prior.map(str::trim).filter(|p| !p.is_empty()) {
        s.push_str("## Previous summary (refresh this)\n\n");
        s.push_str(prior);
        s.push_str("\n\n");
    }
    s.push_str(&diff_digest(diffs));
    s.push_str("\nWrite the refreshed summary.\n");
    s
}

/// A compact, bounded digest of the diff: a full file list with +/- counts,
/// followed by added/removed line bodies up to [`MAX_DIFF_CHARS`].
fn diff_digest(diffs: &[FileDiff<'_>]) -> String {
    let mut s = String::from("## Changed files\n\n");
    for d in diffs.iter().take(MAX_FILES) {
        let (mut added, mut removed) = (0usize, 0usize);
        for h in &d.hunks {
            for line in &h.lines {
                match line.line_type {
                    DiffLineType::Added => added += 1,
                    DiffLineType::Removed => removed += 1,
                    _ => {}
                }
            }
        }
        s.push_str(&format!("- `{}` (+{added}/-{removed})\n", d.path()));
    }
    if diffs.len() > MAX_FILES {
        s.push_str(&format!("- … and {} more files\n", diffs.len() - MAX_FILES));
    }

    s.push_str("\n## Changes\n\n");
    let mut budget = MAX_DIFF_CHARS;
    let mut truncated = false;
    'outer: for d in diffs {
        let header = format!("### {}\n", d.path());
        if header.len() >= budget {
            truncated = true;
            break;
        }
        s.push_str(&header);
        budget -= header.len();
        for h in &d.hunks {
            for line in &h.lines {
                let prefix = match line.line_type {
                    DiffLineType::Added => "+",
                    DiffLineType::Removed => "-",
                    _ => continue,
                };
                let entry = format!("{prefix}{}\n", line.content.trim_end());
                if entry.len() >= budget {
                    truncated = true;
                    break 'outer;
                }
                s.push_str(&entry);
                budget -= entry.len();
            }
        }
    }
    if truncated {
        s.push_str("\n[diff truncated for length]\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::{DiffLine, FileDiff, Hunk};
    use std::borrow::Cow;

    fn line(t: DiffLineType, content: &str) -> DiffLine<'static> {
        DiffLine {
            line_type: t,
            content: Cow::Owned(content.to_string()),
            old_line_no: None,
            new_line_no: None,
        }
    }

    fn diff(path: &str, lines: Vec<DiffLine<'static>>) -> FileDiff<'static> {
        FileDiff {
            old_path: path.to_string(),
            new_path: path.to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                header: None,
                lines,
            }],
        }
    }

    #[test]
    fn digest_lists_files_with_counts_and_bodies() {
        let diffs = vec![diff(
            "src/billing.rs",
            vec![
                line(DiffLineType::Added, "let total = cents * qty;"),
                line(DiffLineType::Removed, "let total = price * qty;"),
                line(DiffLineType::Context, "return total;"),
            ],
        )];
        let prompt = build_summary_user_prompt(&diffs, None);
        assert!(prompt.contains("`src/billing.rs` (+1/-1)"));
        assert!(prompt.contains("+let total = cents * qty;"));
        assert!(prompt.contains("-let total = price * qty;"));
        // Context lines are not echoed into the change body.
        assert!(!prompt.contains("+return total;"));
        assert!(prompt.contains("Write the refreshed summary."));
    }

    #[test]
    fn prior_summary_is_included_when_present() {
        let diffs = vec![diff("a.rs", vec![line(DiffLineType::Added, "x")])];
        let prompt = build_summary_user_prompt(&diffs, Some("Adds retry logic."));
        assert!(prompt.contains("Previous summary"));
        assert!(prompt.contains("Adds retry logic."));
    }

    #[test]
    fn blank_prior_summary_is_omitted() {
        let diffs = vec![diff("a.rs", vec![line(DiffLineType::Added, "x")])];
        let prompt = build_summary_user_prompt(&diffs, Some("   "));
        assert!(!prompt.contains("Previous summary"));
    }

    #[test]
    fn long_diffs_are_truncated() {
        let big: Vec<DiffLine<'static>> = (0..2000)
            .map(|i| {
                line(
                    DiffLineType::Added,
                    &format!("line number {i} with some content"),
                )
            })
            .collect();
        let diffs = vec![diff("big.rs", big)];
        let prompt = build_summary_user_prompt(&diffs, None);
        assert!(prompt.contains("[diff truncated for length]"));
        // The whole prompt stays bounded near the budget (+ file list/headers).
        assert!(prompt.len() < MAX_DIFF_CHARS + 2000);
    }
}
