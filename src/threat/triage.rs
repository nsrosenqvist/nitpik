//! LLM-based triage for threat scanner findings.
//!
//! When pattern matching produces findings and an LLM provider is
//! available, this module sends a single-turn triage request asking
//! the LLM to classify each finding as confirmed, dismissed, or
//! downgraded. Fail-open: on any LLM error, the original findings
//! pass through unmodified.

use indexmap::IndexMap;

use crate::models::finding::Severity;
use crate::providers::{ReviewProvider, TriageVerdict as RawVerdict};

use super::scanner::ThreatMatch;

// ── Public API ──────────────────────────────────────────────────────

/// Triage raw threat matches using the LLM.
///
/// Returns the filtered/reclassified matches. On LLM failure, returns
/// the original matches unchanged (fail-open).
pub async fn triage_findings(
    matches: Vec<ThreatMatch>,
    file_contents: &IndexMap<String, String>,
    provider: &dyn ReviewProvider,
) -> Vec<ThreatMatch> {
    let prompt = build_triage_prompt(&matches, file_contents);
    let system = system_prompt();

    let raw = match provider.triage(&system, &prompt).await {
        Ok(v) => v,
        Err(_) => return matches, // fail-open
    };

    let verdicts = normalize_verdicts(raw);
    if verdicts.is_empty() {
        return matches; // fail-open on unparseable response
    }

    apply_verdicts(matches, &verdicts)
}

// ── Prompt construction ─────────────────────────────────────────────

const TRIAGE_SYSTEM_PROMPT: &str = include_str!("triage_prompt.md");

fn system_prompt() -> String {
    TRIAGE_SYSTEM_PROMPT.to_string()
}

fn build_triage_prompt(
    matches: &[ThreatMatch],
    file_contents: &IndexMap<String, String>,
) -> String {
    let mut prompt = String::from("Findings to triage:\n\n");

    for (i, m) in matches.iter().enumerate() {
        prompt.push_str(&format!(
            "Finding #{i}:\n  File: {}\n  Line: {}\n  Rule: {} ({})\n  Matched: `{}`\n",
            m.file, m.line_number, m.rule_id, m.category, m.matched_text,
        ));

        // Include surrounding context from the file if available
        if let Some(content) = file_contents.get(&m.file) {
            let context = extract_context(content, m.line_number, 3);
            if !context.is_empty() {
                prompt.push_str("  Context:\n");
                prompt.push_str(&context);
            }
        }
        prompt.push('\n');
    }

    prompt
}

/// Extract ±n lines of context around a given 1-based line number.
fn extract_context(content: &str, line_no: u32, n: u32) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len() as u32;
    if total == 0 || line_no == 0 {
        return String::new();
    }
    let start = line_no.saturating_sub(n).max(1);
    let end = (line_no + n).min(total);

    let mut out = String::new();
    for i in start..=end {
        if let Some(line) = lines.get((i - 1) as usize) {
            out.push_str(&format!("    {i}: {line}\n"));
        }
    }
    out
}

// ── Verdict normalization ───────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum TriageClassification {
    Confirmed,
    Dismissed,
    Downgraded,
}

#[derive(Debug)]
struct TriageVerdict {
    index: usize,
    classification: TriageClassification,
}

/// Map provider-returned [`RawVerdict`]s onto the internal classification
/// enum. Verdicts with unrecognized classification strings are dropped.
fn normalize_verdicts(raw: Vec<RawVerdict>) -> Vec<TriageVerdict> {
    raw.into_iter()
        .filter_map(|r| {
            let classification = match r.classification.to_lowercase().as_str() {
                "confirmed" => TriageClassification::Confirmed,
                "dismissed" => TriageClassification::Dismissed,
                "downgraded" => TriageClassification::Downgraded,
                _ => return None,
            };
            Some(TriageVerdict {
                index: r.index,
                classification,
            })
        })
        .collect()
}

/// Apply triage verdicts to the raw matches.
///
/// - Confirmed: keep as-is
/// - Dismissed: remove
/// - Downgraded: set severity to Info
/// - No verdict: keep as-is (fail-open)
fn apply_verdicts(mut matches: Vec<ThreatMatch>, verdicts: &[TriageVerdict]) -> Vec<ThreatMatch> {
    // Build a lookup by index
    let mut verdict_map = std::collections::HashMap::new();
    for v in verdicts {
        verdict_map.insert(v.index, &v.classification);
    }

    // Process in reverse so removal indices stay valid
    let mut to_remove = Vec::new();
    for (i, m) in matches.iter_mut().enumerate() {
        if let Some(classification) = verdict_map.get(&i) {
            match classification {
                TriageClassification::Dismissed => to_remove.push(i),
                TriageClassification::Downgraded => m.severity = Severity::Info,
                TriageClassification::Confirmed => {} // keep as-is
            }
        }
        // No verdict → keep as-is (fail-open)
    }

    // Remove dismissed findings (reverse order to preserve indices)
    for i in to_remove.into_iter().rev() {
        matches.remove(i);
    }

    matches
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threat::rules::ThreatCategory;

    fn make_match(index: usize) -> ThreatMatch {
        ThreatMatch {
            rule_id: format!("rule-{index}"),
            rule_description: format!("Test rule {index}"),
            category: ThreatCategory::DangerousApi,
            severity: Severity::Warning,
            file: "test.js".to_string(),
            line_number: (index as u32 + 1) * 10,
            matched_text: format!("match-{index}"),
        }
    }

    fn raw(index: usize, classification: &str) -> RawVerdict {
        RawVerdict {
            index,
            classification: classification.to_string(),
            rationale: None,
        }
    }

    #[test]
    fn normalize_known_classifications() {
        let raw = vec![
            raw(0, "confirmed"),
            raw(1, "dismissed"),
            raw(2, "downgraded"),
        ];
        let verdicts = normalize_verdicts(raw);
        assert_eq!(verdicts.len(), 3);
        assert_eq!(verdicts[0].classification, TriageClassification::Confirmed);
        assert_eq!(verdicts[1].classification, TriageClassification::Dismissed);
        assert_eq!(verdicts[2].classification, TriageClassification::Downgraded);
    }

    #[test]
    fn normalize_drops_unknown_classifications() {
        let raw = vec![raw(0, "confirmed"), raw(1, "wat")];
        let verdicts = normalize_verdicts(raw);
        assert_eq!(verdicts.len(), 1);
        assert_eq!(verdicts[0].index, 0);
    }

    #[test]
    fn normalize_handles_mixed_case() {
        let raw = vec![raw(0, "Confirmed"), raw(1, "DISMISSED")];
        let verdicts = normalize_verdicts(raw);
        assert_eq!(verdicts.len(), 2);
    }

    #[test]
    fn normalize_empty_input() {
        let verdicts = normalize_verdicts(Vec::new());
        assert!(verdicts.is_empty());
    }

    #[test]
    fn apply_verdicts_downgraded() {
        let matches = vec![make_match(0), make_match(1)];
        let verdicts = vec![
            TriageVerdict {
                index: 0,
                classification: TriageClassification::Confirmed,
            },
            TriageVerdict {
                index: 1,
                classification: TriageClassification::Downgraded,
            },
        ];
        let result = apply_verdicts(matches, &verdicts);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].severity, Severity::Warning); // confirmed → unchanged
        assert_eq!(result[1].severity, Severity::Info); // downgraded
    }

    #[test]
    fn apply_verdicts_dismissed() {
        let matches = vec![make_match(0), make_match(1), make_match(2)];
        let verdicts = vec![TriageVerdict {
            index: 1,
            classification: TriageClassification::Dismissed,
        }];
        let result = apply_verdicts(matches, &verdicts);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].rule_id, "rule-0");
        assert_eq!(result[1].rule_id, "rule-2");
    }

    #[test]
    fn build_prompt_includes_context() {
        let m = make_match(0);
        let mut contents = IndexMap::new();
        contents.insert(
            "test.js".to_string(),
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n".to_string(),
        );
        let prompt = build_triage_prompt(&[m], &contents);
        assert!(prompt.contains("Finding #0"));
        assert!(prompt.contains("Context:"));
    }

    #[test]
    fn extract_context_clamps_boundaries() {
        let content = "a\nb\nc";
        let ctx = extract_context(content, 1, 5);
        assert!(ctx.contains("1: a"));
        assert!(ctx.contains("3: c"));
    }
}
