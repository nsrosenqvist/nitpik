//! Prior-feedback retirement: reply to and resolve review threads the
//! latest push has addressed.
//!
//! On each run, nitpik's own open review threads are fetched from the forge
//! and judged against the current diff by the LLM. The platform's "this
//! line is outdated" flag is passed in as a *hint* — the model still decides
//! whether the change actually fixes the finding (an outdated line may have
//! moved without the bug being fixed). Threads judged **addressed** get a
//! short reply (`Addressed in <sha>.`) and are marked resolved.
//!
//! Opt-in (`--resolve-addressed`) and best-effort: any forge/LLM error warns
//! and leaves threads untouched rather than failing the run. Only nitpik's
//! own threads (recognized by the hidden dedup marker) are ever touched.

use std::sync::Arc;

use crate::constants::MAX_AUX_RETRIES;
use crate::forge::{Forge, ReviewThread};
use crate::models::TokenUsage;
use crate::models::diff::{DiffLineType, FileDiff};
use crate::providers::ReviewProvider;
use crate::providers::response::retry_transient;

/// Cap on the prior-finding text shown to the judge per thread.
const MAX_BODY_CHARS: usize = 600;

/// Outcome of a resolution pass.
#[derive(Debug, Default)]
pub struct ResolveOutcome {
    /// File paths of threads that were replied-to and resolved.
    pub resolved: Vec<String>,
    /// nitpik-originated open threads considered this run.
    pub considered: usize,
    pub tokens: TokenUsage,
}

const JUDGE_SYSTEM: &str = "You decide whether a prior code-review finding has been ADDRESSED by the \
current diff. For each finding you are given the original review comment, the current diff for its \
file, and whether the platform marks the comment's anchored line as outdated (the line changed since \
the comment). The outdated flag is a hint that code moved, NOT proof the issue is fixed. \
Mark a finding `addressed` only when the current diff clearly fixes or removes the issue; otherwise \
`open`. Be conservative — when in doubt, answer `open`.\n\n\
## Output Format\n\n\
Return a single JSON array with one entry per finding (0-based index):\n\
- `index`: 0-based position in the input list\n\
- `classification`: exactly one of `\"addressed\"` or `\"open\"`\n\
- `rationale`: one short sentence (\u{2264} 20 words)\n\n\
Return exactly one entry per finding and no commentary outside the JSON.";

/// Fetch nitpik's open review threads, judge them against the current diff,
/// and reply-to + resolve the ones the diff addresses.
pub async fn resolve_addressed_threads(
    provider: &Arc<dyn ReviewProvider>,
    model: &str,
    forge: &dyn Forge,
    diffs: &[FileDiff<'_>],
    head_sha: Option<&str>,
) -> ResolveOutcome {
    let threads = match forge.open_review_threads().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Warning: could not fetch review threads ({e}); skipping resolution.");
            return ResolveOutcome::default();
        }
    };

    // Only ever touch nitpik's own threads (those carrying our marker).
    let threads: Vec<ReviewThread> = threads
        .into_iter()
        .filter(|t| t.fingerprint.is_some())
        .collect();
    if threads.is_empty() {
        return ResolveOutcome::default();
    }

    let user = build_judge_prompt(&threads, diffs);
    let outcome = match retry_transient(MAX_AUX_RETRIES, || {
        provider.triage(model, JUDGE_SYSTEM, &user)
    })
    .await
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("Warning: thread-resolution judge failed ({e}); leaving threads as-is.");
            return ResolveOutcome::default();
        }
    };

    let reply = match head_sha.map(short_sha) {
        Some(sha) => format!("Addressed in {sha}."),
        None => "Addressed in a later commit.".to_string(),
    };

    let mut resolved = Vec::new();
    for v in &outcome.verdicts {
        if !v.classification.eq_ignore_ascii_case("addressed") {
            continue;
        }
        let Some(thread) = threads.get(v.index) else {
            continue;
        };
        match forge.reply_and_resolve(thread, &reply).await {
            Ok(()) => resolved.push(thread.path.clone()),
            Err(e) => eprintln!(
                "Warning: failed to resolve thread on {} ({e}).",
                thread.path
            ),
        }
    }

    ResolveOutcome {
        resolved,
        considered: threads.len(),
        tokens: outcome.tokens,
    }
}

/// Build the judge's user prompt: a numbered list of prior findings, each
/// with the current diff for its file and the outdated hint.
fn build_judge_prompt(threads: &[ReviewThread], diffs: &[FileDiff<'_>]) -> String {
    let mut s = String::from(
        "Prior review findings to judge against the current diff:\n\n",
    );
    for (i, t) in threads.iter().enumerate() {
        s.push_str(&format!("### Finding {i}\n"));
        s.push_str(&format!("- file: `{}`", t.path));
        if let Some(line) = t.line {
            s.push_str(&format!(" (line {line})"));
        }
        s.push('\n');
        s.push_str(&format!(
            "- platform marks anchored line outdated: {}\n",
            if t.outdated { "yes" } else { "no" }
        ));
        s.push_str("- original comment:\n");
        s.push_str(&truncate(&t.body, MAX_BODY_CHARS));
        s.push_str("\n\n- current diff for this file:\n");
        match diffs.iter().find(|d| d.new_path == t.path) {
            Some(d) => {
                s.push_str("```diff\n");
                s.push_str(&render_file_diff(d));
                s.push_str("```\n");
            }
            None => s.push_str("(file not present in the current diff)\n"),
        }
        s.push('\n');
    }
    s.push_str("Return the JSON array described in your instructions.\n");
    s
}

/// Render a file's hunks as `+`/`-`/space-prefixed lines.
fn render_file_diff(diff: &FileDiff<'_>) -> String {
    let mut out = String::new();
    for hunk in &diff.hunks {
        for line in &hunk.lines {
            let prefix = match line.line_type {
                DiffLineType::Added => '+',
                DiffLineType::Removed => '-',
                DiffLineType::Context => ' ',
            };
            out.push(prefix);
            out.push_str(&line.content);
            if !line.content.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

/// Shorten a commit SHA to 7 chars for the reply.
fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Truncate on a char boundary, marking the cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::{ForgeError, PullRequest, ReviewDraft};
    use crate::models::diff::{DiffLine, Hunk};
    use crate::providers::{ReviewOutcome, TriageOutcome, TriageVerdict};
    use std::borrow::Cow;
    use std::sync::Mutex;

    fn thread(idx: usize, path: &str, outdated: bool) -> ReviewThread {
        ReviewThread {
            id: format!("T{idx}"),
            root_comment_id: Some(idx as u64),
            fingerprint: Some(format!("fp{idx}")),
            path: path.to_string(),
            line: Some(3),
            outdated,
            body: format!("🔴 Finding {idx}\n\n<!-- nitpik:fp{idx} -->"),
        }
    }

    fn diff(path: &str) -> FileDiff<'static> {
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
                lines: vec![
                    DiffLine {
                        line_type: DiffLineType::Removed,
                        content: Cow::Borrowed("bad()"),
                        old_line_no: Some(1),
                        new_line_no: None,
                    },
                    DiffLine {
                        line_type: DiffLineType::Added,
                        content: Cow::Borrowed("good()"),
                        old_line_no: None,
                        new_line_no: Some(1),
                    },
                ],
            }],
        }
    }

    #[test]
    fn judge_prompt_has_finding_diff_and_outdated_hint() {
        let threads = vec![thread(0, "db.py", true)];
        let p = build_judge_prompt(&threads, &[diff("db.py")]);
        assert!(p.contains("### Finding 0"));
        assert!(p.contains("`db.py`"));
        assert!(p.contains("outdated: yes"));
        assert!(p.contains("+good()"));
        assert!(p.contains("-bad()"));
    }

    #[test]
    fn judge_prompt_notes_file_absent_from_diff() {
        let p = build_judge_prompt(&[thread(0, "gone.py", false)], &[diff("other.py")]);
        assert!(p.contains("not present in the current diff"));
        assert!(p.contains("outdated: no"));
    }

    #[test]
    fn short_sha_caps_to_seven() {
        assert_eq!(short_sha("0123456789abcdef"), "0123456");
        assert_eq!(short_sha("abc"), "abc");
    }

    // --- mock forge recording reply_and_resolve calls ---

    struct MockForge {
        threads: Vec<ReviewThread>,
        resolved: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Forge for MockForge {
        async fn pull_request(&self) -> Result<PullRequest, ForgeError> {
            unreachable!()
        }
        async fn existing_review_comments(
            &self,
        ) -> Result<Vec<crate::forge::ExistingComment>, ForgeError> {
            Ok(Vec::new())
        }
        async fn post_review(&self, _draft: &ReviewDraft) -> Result<(), ForgeError> {
            unreachable!()
        }
        async fn open_review_threads(&self) -> Result<Vec<ReviewThread>, ForgeError> {
            Ok(self.threads.clone())
        }
        async fn reply_and_resolve(
            &self,
            t: &ReviewThread,
            _reply: &str,
        ) -> Result<(), ForgeError> {
            self.resolved.lock().unwrap().push(t.id.clone());
            Ok(())
        }
    }

    struct MockJudge(TriageOutcome);

    #[async_trait::async_trait]
    impl ReviewProvider for MockJudge {
        async fn review(
            &self,
            _a: &crate::models::AgentDefinition,
            _p: &str,
            _ag: bool,
            _mt: usize,
            _mc: usize,
        ) -> Result<ReviewOutcome, crate::providers::ProviderError> {
            unreachable!()
        }
        async fn triage(
            &self,
            _m: &str,
            _s: &str,
            _u: &str,
        ) -> Result<TriageOutcome, crate::providers::ProviderError> {
            Ok(self.0.clone())
        }
    }

    fn verdict(index: usize, classification: &str) -> TriageVerdict {
        TriageVerdict {
            index,
            classification: classification.into(),
            rationale: None,
        }
    }

    #[tokio::test]
    async fn resolves_only_addressed_threads() {
        let forge = MockForge {
            threads: vec![thread(0, "a.py", true), thread(1, "b.py", false)],
            resolved: Mutex::new(Vec::new()),
        };
        let provider: Arc<dyn ReviewProvider> = Arc::new(MockJudge(TriageOutcome {
            verdicts: vec![verdict(0, "addressed"), verdict(1, "open")],
            tokens: TokenUsage::default(),
        }));
        let diffs = vec![diff("a.py"), diff("b.py")];
        let out = resolve_addressed_threads(&provider, "m", &forge, &diffs, Some("deadbeefcafe")).await;
        assert_eq!(out.considered, 2);
        assert_eq!(out.resolved, vec!["a.py".to_string()]);
        // Only thread T0 was resolved on the forge.
        assert_eq!(*forge.resolved.lock().unwrap(), vec!["T0".to_string()]);
    }

    #[tokio::test]
    async fn ignores_non_nitpik_threads() {
        // A thread without a fingerprint (a human's thread) is never judged.
        let mut human = thread(0, "a.py", true);
        human.fingerprint = None;
        let forge = MockForge {
            threads: vec![human],
            resolved: Mutex::new(Vec::new()),
        };
        let provider: Arc<dyn ReviewProvider> = Arc::new(MockJudge(TriageOutcome {
            verdicts: vec![verdict(0, "addressed")],
            tokens: TokenUsage::default(),
        }));
        let out = resolve_addressed_threads(&provider, "m", &forge, &[diff("a.py")], None).await;
        assert_eq!(out.considered, 0);
        assert!(out.resolved.is_empty());
        assert!(forge.resolved.lock().unwrap().is_empty());
    }
}
