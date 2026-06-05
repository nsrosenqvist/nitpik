//! Critic / verification pass.
//!
//! Suppresses probable false positives produced by upstream reviewers.
//! Opt-in via `--verify`.
//!
//! The default path ([`verify_findings_panel`]) is **perspective-diverse**:
//! it runs three independent critic lenses — the balanced `critic` plus a
//! `critic-soundness` lens (is the defect logically real?) and a
//! `critic-grounding` lens (is it anchored in the real diff?) — in parallel
//! and combines their keep/drop verdicts by majority vote, requiring
//! unanimity to drop a cross-lens-corroborated finding. Diversity across
//! lenses catches the blind spots a single critic prompt shares with
//! itself. [`verify_findings`] keeps the original single-critic behavior as
//! a building block and graceful fallback.
//!
//! Every lens uses the same [`ReviewProvider::triage`] machinery already in
//! use for threat triage — single round, structured JSON output, no tools.
//! The verdict format intentionally mirrors [`TriageVerdict`] so we reuse
//! the parser path.

use std::collections::HashMap;
use std::sync::Arc;

use crate::models::TokenUsage;
use crate::models::finding::Finding;
#[cfg(test)]
use crate::providers::ProviderError;
use crate::providers::{ReviewProvider, TriageOutcome};

/// Outcome of a verify pass.
#[derive(Debug, Clone)]
pub struct VerifyOutcome {
    /// Findings the critic kept (subset of input, original order).
    pub kept: Vec<Finding>,
    /// Findings the critic dropped, paired with the critic's reason.
    pub dropped: Vec<DroppedFinding>,
    /// Tokens consumed by the critic call.
    pub tokens: TokenUsage,
}

/// A finding the critic voted to drop, with its rationale.
#[derive(Debug, Clone)]
pub struct DroppedFinding {
    pub finding: Finding,
    pub reason: String,
}

/// Run the critic pass over `findings`. The function fails open: any
/// provider error returns the original findings unchanged so the
/// reviewer pipeline does not break on a transient critic outage.
///
/// `findings` is consumed; on success the returned `kept` vec is in
/// the original order.
///
/// `corroboration` maps each finding's [`dedup::fingerprint`] to the
/// number of distinct reviewer lenses that independently raised it (see
/// [`crate::orchestrator::dedup::deduplicate_with_corroboration`]).
/// Findings flagged by 2+ lenses are surfaced to the critic as
/// corroborated so it can apply a stronger keep-bias to them. Pass an
/// empty map to disable the signal.
pub async fn verify_findings(
    provider: &Arc<dyn ReviewProvider>,
    model: &str,
    findings: Vec<Finding>,
    corroboration: &HashMap<String, u32>,
) -> VerifyOutcome {
    if findings.is_empty() {
        return VerifyOutcome {
            kept: findings,
            dropped: Vec::new(),
            tokens: TokenUsage::default(),
        };
    }

    let critic = match crate::agents::builtin::get_builtin("critic") {
        Some(a) => a,
        None => {
            return VerifyOutcome {
                kept: findings,
                dropped: Vec::new(),
                tokens: TokenUsage::default(),
            };
        }
    };

    let user_prompt = build_critic_prompt(&findings, corroboration);

    match crate::providers::response::retry_transient(crate::constants::MAX_AUX_RETRIES, || {
        provider.triage(model, &critic.system_prompt, &user_prompt)
    })
    .await
    {
        Ok(outcome) => apply_verdicts(findings, outcome),
        Err(err) => {
            // Fail open: log to stderr and keep everything.
            eprintln!("Warning: critic pass failed ({err}); keeping all findings.");
            VerifyOutcome {
                kept: findings,
                dropped: Vec::new(),
                tokens: TokenUsage::default(),
            }
        }
    }
}

/// Built-in critic lenses composing the perspective-diverse verify panel:
/// the balanced [`critic`](crate::agents::builtin) plus two complementary
/// single-axis lenses. Their keep/drop verdicts are combined by
/// [`combine_panel`].
const PANEL_LENSES: [&str; 3] = ["critic", "critic-soundness", "critic-grounding"];

/// Perspective-diverse verification: run several independent critic lenses
/// over the findings and combine their keep/drop verdicts by majority vote.
///
/// A finding is dropped only when a **majority** of the lenses that
/// answered vote drop; a finding corroborated by 2+ independent reviewer
/// lenses (per `corroboration`) requires a **unanimous** drop, so genuine
/// cross-lens agreement is hard to overturn. Running diverse lenses and
/// voting catches the blind spots a single critic prompt would share with
/// itself — the same "0-or-2+, never exactly 1" logic the corroboration
/// signal applies on the production side, applied here on the filtering
/// side.
///
/// Fails open at every level: a lens that errors abstains from the vote,
/// and if every lens errors all findings are kept. Falls back to the
/// single-critic [`verify_findings`] when a panel profile can't be loaded.
pub async fn verify_findings_panel(
    provider: &Arc<dyn ReviewProvider>,
    model: &str,
    findings: Vec<Finding>,
    corroboration: &HashMap<String, u32>,
) -> VerifyOutcome {
    if findings.is_empty() {
        return VerifyOutcome {
            kept: findings,
            dropped: Vec::new(),
            tokens: TokenUsage::default(),
        };
    }

    // Load the lens system prompts; if any is missing, degrade to the
    // single-critic pass rather than running a partial panel.
    let mut prompts = Vec::with_capacity(PANEL_LENSES.len());
    for name in PANEL_LENSES {
        match crate::agents::builtin::get_builtin(name) {
            Some(a) => prompts.push(a.system_prompt),
            None => return verify_findings(provider, model, findings, corroboration).await,
        }
    }

    let user_prompt = build_critic_prompt(&findings, corroboration);

    // Run the lenses concurrently on this task — no spawn, so the provider
    // reference is simply shared across the three futures.
    let (a, b, c) = tokio::join!(
        run_one_critic(provider, model, &prompts[0], &user_prompt),
        run_one_critic(provider, model, &prompts[1], &user_prompt),
        run_one_critic(provider, model, &prompts[2], &user_prompt),
    );

    let mut tokens = TokenUsage::default();
    let mut lens_verdicts: Vec<Vec<crate::providers::TriageVerdict>> = Vec::new();
    for (verdicts, lens_tokens) in [a, b, c].into_iter().flatten() {
        tokens += lens_tokens;
        lens_verdicts.push(verdicts);
    }

    if lens_verdicts.is_empty() {
        // Every lens failed — fail open, keep everything.
        eprintln!("Warning: all critic lenses failed; keeping all findings.");
        return VerifyOutcome {
            kept: findings,
            dropped: Vec::new(),
            tokens,
        };
    }

    let (kept, dropped) = combine_panel(findings, &lens_verdicts, corroboration);
    VerifyOutcome {
        kept,
        dropped,
        tokens,
    }
}

/// Run a single critic lens and return its per-finding verdicts, or `None`
/// when the call fails after retries (the lens abstains from the vote).
async fn run_one_critic(
    provider: &Arc<dyn ReviewProvider>,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Option<(Vec<crate::providers::TriageVerdict>, TokenUsage)> {
    match crate::providers::response::retry_transient(crate::constants::MAX_AUX_RETRIES, || {
        provider.triage(model, system_prompt, user_prompt)
    })
    .await
    {
        Ok(outcome) => Some((outcome.verdicts, outcome.tokens)),
        Err(err) => {
            eprintln!("Warning: a critic lens failed ({err}); it abstains from the vote.");
            None
        }
    }
}

/// Combine per-lens verdicts into kept/dropped sets by majority vote.
///
/// `lens_verdicts` holds one verdict list per lens that answered (failed
/// lenses are already excluded). A finding is dropped when the number of
/// lenses voting drop reaches the threshold: a strict majority normally,
/// or **all** answering lenses when the finding is corroborated by 2+
/// reviewer lenses. A finding with no matching verdict from any lens
/// defaults to keep (fail open).
fn combine_panel(
    findings: Vec<Finding>,
    lens_verdicts: &[Vec<crate::providers::TriageVerdict>],
    corroboration: &HashMap<String, u32>,
) -> (Vec<Finding>, Vec<DroppedFinding>) {
    use crate::orchestrator::dedup::fingerprint;

    let n = lens_verdicts.len();
    if n == 0 {
        return (findings, Vec::new());
    }

    let mut kept = Vec::new();
    let mut dropped = Vec::new();

    for (i, finding) in findings.into_iter().enumerate() {
        // Collect this finding's drop rationales across the answering lenses.
        let drop_reasons: Vec<String> = lens_verdicts
            .iter()
            .filter_map(|verdicts| verdicts.iter().find(|v| v.index == i))
            .filter(|v| v.classification.eq_ignore_ascii_case("drop"))
            .map(|v| {
                v.rationale
                    .clone()
                    .unwrap_or_else(|| "(no reason provided)".into())
            })
            .collect();

        let corroborated = corroboration
            .get(&fingerprint(&finding))
            .is_some_and(|&c| c >= 2);
        // Strict majority of the answering lenses; unanimity for
        // corroborated findings.
        let threshold = if corroborated { n } else { n / 2 + 1 };

        if drop_reasons.len() >= threshold {
            let reason = drop_reasons.join("; ");
            dropped.push(DroppedFinding { finding, reason });
        } else {
            kept.push(finding);
        }
    }

    (kept, dropped)
}

/// Build the critic's user prompt: a numbered list of findings, each
/// rendered compactly so the model can scan the batch and produce
/// per-index verdicts.
fn build_critic_prompt(findings: &[Finding], corroboration: &HashMap<String, u32>) -> String {
    use crate::orchestrator::dedup::fingerprint;
    let mut s = String::with_capacity(findings.len() * 200);
    s.push_str(
        "You are reviewing the following candidate findings. Vote keep or drop on each one. \
         Return one JSON object per finding, indexed exactly as listed below.\n\n\
         A `corroboration` field, when present, means that many *independent* reviewer lenses \
         raised this issue separately — strong evidence it is real. Apply a heavy keep-bias to \
         corroborated findings; drop one only with a specific, concrete reason.\n\n",
    );
    for (i, f) in findings.iter().enumerate() {
        s.push_str(&format!(
            "### Finding {i}\n\
             - file: `{file}`\n\
             - line: {line}\n\
             - severity: {sev}\n\
             - title: {title}\n\
             - message: {msg}\n",
            i = i,
            file = f.file,
            line = f.line,
            sev = f.severity,
            title = f.title,
            msg = f.message,
        ));
        if let Some(ref sug) = f.suggestion {
            s.push_str(&format!("- suggestion: {sug}\n"));
        }
        if !f.evidence.is_empty() {
            s.push_str(&format!("- evidence: {}\n", f.evidence.join(", ")));
        }
        if let Some(&count) = corroboration.get(&fingerprint(f))
            && count >= 2
        {
            s.push_str(&format!(
                "- corroboration: independently flagged by {count} reviewers\n"
            ));
        }
        s.push('\n');
    }
    s.push_str(
        "Return only the JSON array described in your instructions. \
         Use the same indices shown above.\n",
    );
    s
}

/// Apply the critic's verdicts to the candidate findings.
///
/// Verdicts whose `classification` is `"drop"` (case-insensitive) are
/// excluded; everything else is kept (including malformed verdicts —
/// fail open). Findings without any matching verdict default to
/// **keep**.
fn apply_verdicts(findings: Vec<Finding>, outcome: TriageOutcome) -> VerifyOutcome {
    use std::collections::HashMap;
    let mut by_index: HashMap<usize, &crate::providers::TriageVerdict> = HashMap::new();
    for v in &outcome.verdicts {
        by_index.insert(v.index, v);
    }

    let mut kept = Vec::with_capacity(findings.len());
    let mut dropped = Vec::new();

    for (i, finding) in findings.into_iter().enumerate() {
        let drop_it = by_index
            .get(&i)
            .is_some_and(|v| v.classification.eq_ignore_ascii_case("drop"));
        if drop_it {
            let reason = by_index
                .get(&i)
                .and_then(|v| v.rationale.clone())
                .unwrap_or_else(|| "(no reason provided)".to_string());
            dropped.push(DroppedFinding { finding, reason });
        } else {
            kept.push(finding);
        }
    }

    VerifyOutcome {
        kept,
        dropped,
        tokens: outcome.tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;
    use crate::providers::TriageVerdict;

    fn f(title: &str) -> Finding {
        Finding {
            file: "x.rs".into(),
            line: 1,
            end_line: None,
            severity: Severity::Warning,
            title: title.into(),
            message: "m".into(),
            suggestion: None,
            agent: "test".into(),
            evidence: Vec::new(),
        }
    }

    fn verdict(idx: usize, cls: &str, reason: Option<&str>) -> TriageVerdict {
        TriageVerdict {
            index: idx,
            classification: cls.into(),
            rationale: reason.map(String::from),
        }
    }

    #[test]
    fn drop_verdict_removes_finding_from_kept() {
        let findings = vec![f("a"), f("b"), f("c")];
        let outcome = TriageOutcome {
            verdicts: vec![
                verdict(0, "keep", None),
                verdict(1, "drop", Some("speculative")),
                verdict(2, "keep", None),
            ],
            tokens: TokenUsage::default(),
        };
        let result = apply_verdicts(findings, outcome);
        assert_eq!(result.kept.len(), 2);
        assert_eq!(result.kept[0].title, "a");
        assert_eq!(result.kept[1].title, "c");
        assert_eq!(result.dropped.len(), 1);
        assert_eq!(result.dropped[0].finding.title, "b");
        assert_eq!(result.dropped[0].reason, "speculative");
    }

    #[test]
    fn missing_verdict_defaults_to_keep() {
        let findings = vec![f("a"), f("b")];
        let outcome = TriageOutcome {
            verdicts: vec![verdict(0, "drop", None)],
            tokens: TokenUsage::default(),
        };
        let result = apply_verdicts(findings, outcome);
        assert_eq!(result.kept.len(), 1);
        assert_eq!(result.kept[0].title, "b");
    }

    #[test]
    fn drop_classification_is_case_insensitive() {
        let findings = vec![f("a")];
        let outcome = TriageOutcome {
            verdicts: vec![verdict(0, "DROP", None)],
            tokens: TokenUsage::default(),
        };
        let result = apply_verdicts(findings, outcome);
        assert!(result.kept.is_empty());
        assert_eq!(result.dropped.len(), 1);
    }

    #[test]
    fn unknown_verdict_keeps_finding() {
        let findings = vec![f("a")];
        let outcome = TriageOutcome {
            verdicts: vec![verdict(0, "maybe", None)],
            tokens: TokenUsage::default(),
        };
        let result = apply_verdicts(findings, outcome);
        assert_eq!(result.kept.len(), 1);
        assert!(result.dropped.is_empty());
    }

    #[test]
    fn build_prompt_includes_each_finding_with_index() {
        let findings = vec![f("first"), f("second")];
        let p = build_critic_prompt(&findings, &HashMap::new());
        assert!(p.contains("### Finding 0"));
        assert!(p.contains("### Finding 1"));
        assert!(p.contains("first"));
        assert!(p.contains("second"));
    }

    #[test]
    fn build_prompt_surfaces_corroboration_only_when_two_or_more() {
        use crate::orchestrator::dedup::fingerprint;
        let solo = f("solo lens");
        let agreed = f("two lenses agreed");
        let mut corroboration = HashMap::new();
        corroboration.insert(fingerprint(&solo), 1);
        corroboration.insert(fingerprint(&agreed), 3);

        let p = build_critic_prompt(&[solo, agreed], &corroboration);
        // The 3-lens finding is surfaced as corroborated…
        assert!(p.contains("independently flagged by 3 reviewers"));
        // …but a single-lens finding gets no corroboration line.
        assert!(!p.contains("flagged by 1 reviewers"));
        // And the system-facing explanation of the field is present.
        assert!(p.contains("independent"));
    }

    // ── Perspective-diverse panel ───────────────────────────────────

    /// One lens's verdict list: `(index, drop?)` pairs.
    fn lens(votes: &[(usize, bool)]) -> Vec<TriageVerdict> {
        votes
            .iter()
            .map(|&(i, drop)| verdict(i, if drop { "drop" } else { "keep" }, Some("r")))
            .collect()
    }

    #[test]
    fn combine_panel_drops_on_majority_keeps_on_minority() {
        // index 0: 2 of 3 drop → dropped. index 1: 1 of 3 drop → kept.
        let findings = vec![f("a"), f("b")];
        let lenses = vec![
            lens(&[(0, true), (1, false)]),
            lens(&[(0, true), (1, true)]),
            lens(&[(0, false), (1, false)]),
        ];
        let (kept, dropped) = combine_panel(findings, &lenses, &HashMap::new());
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].finding.title, "a");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "b");
    }

    #[test]
    fn combine_panel_corroborated_finding_needs_unanimous_drop() {
        use crate::orchestrator::dedup::fingerprint;
        let corro_finding = f("corroborated");
        let solo_finding = f("solo");
        let mut corroboration = HashMap::new();
        corroboration.insert(fingerprint(&corro_finding), 3);
        // Both findings get a 2-of-3 drop vote.
        let lenses = vec![
            lens(&[(0, true), (1, true)]),
            lens(&[(0, true), (1, true)]),
            lens(&[(0, false), (1, false)]),
        ];
        let (kept, dropped) =
            combine_panel(vec![corro_finding, solo_finding], &lenses, &corroboration);
        // The corroborated one survives (needs 3/3); the solo one is dropped (2/3).
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "corroborated");
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].finding.title, "solo");
    }

    #[test]
    fn combine_panel_corroborated_dropped_only_when_all_agree() {
        use crate::orchestrator::dedup::fingerprint;
        let finding = f("corroborated");
        let mut corroboration = HashMap::new();
        corroboration.insert(fingerprint(&finding), 2);
        let lenses = vec![lens(&[(0, true)]), lens(&[(0, true)]), lens(&[(0, true)])];
        let (kept, dropped) = combine_panel(vec![finding], &lenses, &corroboration);
        assert!(kept.is_empty());
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn combine_panel_single_lens_decides() {
        // With only one answering lens, its verdict is the majority.
        let lenses = vec![lens(&[(0, true), (1, false)])];
        let (kept, dropped) = combine_panel(vec![f("a"), f("b")], &lenses, &HashMap::new());
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].finding.title, "a");
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn combine_panel_no_lenses_keeps_all() {
        let (kept, dropped) = combine_panel(vec![f("a"), f("b")], &[], &HashMap::new());
        assert_eq!(kept.len(), 2);
        assert!(dropped.is_empty());
    }

    #[test]
    fn combine_panel_concatenates_drop_reasons() {
        let lenses = vec![
            vec![verdict(0, "drop", Some("hallucinated symbol"))],
            vec![verdict(0, "drop", Some("not reachable"))],
            vec![verdict(0, "keep", None)],
        ];
        let (_, dropped) = combine_panel(vec![f("a")], &lenses, &HashMap::new());
        assert_eq!(dropped.len(), 1);
        assert!(dropped[0].reason.contains("hallucinated symbol"));
        assert!(dropped[0].reason.contains("not reachable"));
    }

    #[tokio::test]
    async fn verify_panel_drops_when_lenses_agree() {
        // MockTriage returns the same verdicts for every lens, so all three
        // agree — index 1 is unanimously dropped.
        let provider: Arc<dyn ReviewProvider> = Arc::new(MockTriage(TriageOutcome {
            verdicts: vec![verdict(0, "keep", None), verdict(1, "drop", Some("nope"))],
            tokens: TokenUsage::default(),
        }));
        let result =
            verify_findings_panel(&provider, "m", vec![f("a"), f("b")], &HashMap::new()).await;
        assert_eq!(result.kept.len(), 1);
        assert_eq!(result.kept[0].title, "a");
        assert_eq!(result.dropped.len(), 1);
        assert_eq!(result.dropped[0].finding.title, "b");
    }

    #[tokio::test]
    async fn verify_panel_fails_open_when_all_lenses_error() {
        let provider: Arc<dyn ReviewProvider> = Arc::new(FailingTriage);
        let result =
            verify_findings_panel(&provider, "m", vec![f("a"), f("b")], &HashMap::new()).await;
        assert_eq!(result.kept.len(), 2);
        assert!(result.dropped.is_empty());
    }

    #[tokio::test]
    async fn verify_panel_passes_through_empty_input() {
        let provider: Arc<dyn ReviewProvider> = Arc::new(FailingTriage);
        let result = verify_findings_panel(&provider, "m", Vec::new(), &HashMap::new()).await;
        assert!(result.kept.is_empty());
    }

    /// Mock provider that returns a canned triage outcome.
    struct MockTriage(TriageOutcome);

    #[async_trait::async_trait]
    impl ReviewProvider for MockTriage {
        async fn review(
            &self,
            _agent: &crate::models::AgentDefinition,
            _prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> Result<crate::providers::ReviewOutcome, ProviderError> {
            unreachable!()
        }

        async fn triage(
            &self,
            _model: &str,
            _system_prompt: &str,
            _user_prompt: &str,
        ) -> Result<TriageOutcome, ProviderError> {
            Ok(self.0.clone())
        }
    }

    /// Mock provider whose triage always errors — exercise the
    /// fail-open path.
    struct FailingTriage;

    #[async_trait::async_trait]
    impl ReviewProvider for FailingTriage {
        async fn review(
            &self,
            _agent: &crate::models::AgentDefinition,
            _prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> Result<crate::providers::ReviewOutcome, ProviderError> {
            unreachable!()
        }

        async fn triage(
            &self,
            _model: &str,
            _system_prompt: &str,
            _user_prompt: &str,
        ) -> Result<TriageOutcome, ProviderError> {
            Err(ProviderError::ApiError("boom".into()))
        }
    }

    #[tokio::test]
    async fn verify_findings_drops_per_critic_verdict() {
        let provider: Arc<dyn ReviewProvider> = Arc::new(MockTriage(TriageOutcome {
            verdicts: vec![verdict(0, "keep", None), verdict(1, "drop", Some("nope"))],
            tokens: TokenUsage::default(),
        }));
        let result = verify_findings(
            &provider,
            "test-model",
            vec![f("a"), f("b")],
            &HashMap::new(),
        )
        .await;
        assert_eq!(result.kept.len(), 1);
        assert_eq!(result.kept[0].title, "a");
        assert_eq!(result.dropped.len(), 1);
        assert_eq!(result.dropped[0].finding.title, "b");
    }

    #[tokio::test]
    async fn verify_findings_fails_open_on_provider_error() {
        let provider: Arc<dyn ReviewProvider> = Arc::new(FailingTriage);
        let result = verify_findings(
            &provider,
            "test-model",
            vec![f("a"), f("b")],
            &HashMap::new(),
        )
        .await;
        assert_eq!(result.kept.len(), 2);
        assert!(result.dropped.is_empty());
    }

    #[tokio::test]
    async fn verify_findings_passes_through_empty_input() {
        let provider: Arc<dyn ReviewProvider> = Arc::new(FailingTriage);
        let result = verify_findings(&provider, "test-model", Vec::new(), &HashMap::new()).await;
        assert!(result.kept.is_empty());
        assert!(result.dropped.is_empty());
    }
}
