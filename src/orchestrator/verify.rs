//! Critic / verification pass.
//!
//! Runs a single-turn LLM call per file with findings, asking a
//! built-in `critic` profile to vote keep/drop on each finding. Used
//! to suppress probable false positives produced by upstream
//! reviewers. Opt-in via `--verify`.
//!
//! Calls the same [`ReviewProvider::triage`] machinery already in use
//! for threat triage — single round, structured JSON output, no
//! tools. The critic's verdict format intentionally mirrors
//! [`TriageVerdict`] so we can reuse the parser path.

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
pub async fn verify_findings(
    provider: &Arc<dyn ReviewProvider>,
    model: &str,
    findings: Vec<Finding>,
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

    let user_prompt = build_critic_prompt(&findings);

    match provider
        .triage(model, &critic.system_prompt, &user_prompt)
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

/// Build the critic's user prompt: a numbered list of findings, each
/// rendered compactly so the model can scan the batch and produce
/// per-index verdicts.
fn build_critic_prompt(findings: &[Finding]) -> String {
    let mut s = String::with_capacity(findings.len() * 200);
    s.push_str(
        "You are reviewing the following candidate findings. Vote keep or drop on each one. \
         Return one JSON object per finding, indexed exactly as listed below.\n\n",
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
        let p = build_critic_prompt(&findings);
        assert!(p.contains("### Finding 0"));
        assert!(p.contains("### Finding 1"));
        assert!(p.contains("first"));
        assert!(p.contains("second"));
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
        let result = verify_findings(&provider, "test-model", vec![f("a"), f("b")]).await;
        assert_eq!(result.kept.len(), 1);
        assert_eq!(result.kept[0].title, "a");
        assert_eq!(result.dropped.len(), 1);
        assert_eq!(result.dropped[0].finding.title, "b");
    }

    #[tokio::test]
    async fn verify_findings_fails_open_on_provider_error() {
        let provider: Arc<dyn ReviewProvider> = Arc::new(FailingTriage);
        let result = verify_findings(&provider, "test-model", vec![f("a"), f("b")]).await;
        assert_eq!(result.kept.len(), 2);
        assert!(result.dropped.is_empty());
    }

    #[tokio::test]
    async fn verify_findings_passes_through_empty_input() {
        let provider: Arc<dyn ReviewProvider> = Arc::new(FailingTriage);
        let result = verify_findings(&provider, "test-model", Vec::new()).await;
        assert!(result.kept.is_empty());
        assert!(result.dropped.is_empty());
    }
}
