//! Threat scanner: harmful code detection in diffs.
//!
//! # Bounded Context: Threat Scanning
//!
//! Hybrid pipeline that detects potentially harmful code patterns
//! (obfuscated payloads, dangerous API usage, supply chain hooks,
//! backdoors, data exfiltration) using regex + entropy + structural
//! heuristics. When findings exist and an LLM provider is available,
//! a dedicated triage call reclassifies each finding as confirmed,
//! dismissed, or downgraded.

pub mod rules;
pub mod scanner;
pub mod triage;

use indexmap::IndexMap;

use crate::constants::THREAT_SCANNER_AGENT;
use crate::models::diff::FileDiff;
use crate::models::finding::{Finding, Severity};
use crate::providers::ReviewProvider;

use rules::ThreatRule;
use scanner::ThreatMatch;

/// Convert a `ThreatMatch` into a user-facing `Finding`.
pub fn match_to_finding(m: &ThreatMatch) -> Finding {
    Finding {
        file: m.file.clone(),
        line: m.line_number,
        end_line: None,
        severity: m.severity,
        title: format!("{}: {}", m.category, m.rule_description),
        message: format!(
            "Threat pattern detected (rule: {}, category: {}). Matched: `{}`",
            m.rule_id,
            m.category,
            truncate_match(&m.matched_text, 120),
        ),
        suggestion: Some(suggestion_for_severity(m.severity)),
        agent: THREAT_SCANNER_AGENT.to_string(),
    }
}

/// Truncate matched text for display.
fn truncate_match(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}…", &text[..max_len])
    }
}

/// Provide a severity-appropriate suggestion.
fn suggestion_for_severity(severity: Severity) -> String {
    match severity {
        Severity::Error => "This pattern is strongly associated with malicious code. \
             Investigate immediately and remove if not legitimate."
            .to_string(),
        Severity::Warning => "Review this pattern carefully. If it is intentional and safe, \
             consider adding a `// nosec` comment to suppress future alerts."
            .to_string(),
        Severity::Info => "This pattern was noted but may be benign. Verify it is \
             used safely in context."
            .to_string(),
    }
}

/// Run the threat scanning pipeline and produce findings.
///
/// 1. Runs regex + entropy pattern matching (line-scope + file-scope).
/// 2. If findings exist and a provider is given, runs LLM triage.
/// 3. Converts surviving matches into `Finding` values.
pub async fn scan_for_threats(
    diffs: &[FileDiff<'_>],
    file_contents: &IndexMap<String, String>,
    rules: &[ThreatRule],
    provider: Option<&dyn ReviewProvider>,
) -> Vec<Finding> {
    let raw_matches = scanner::scan_for_threats(diffs, file_contents, rules);

    if raw_matches.is_empty() {
        return Vec::new();
    }

    let triaged = if let Some(provider) = provider {
        triage::triage_findings(raw_matches, file_contents, provider).await
    } else {
        raw_matches
    };

    triaged.iter().map(match_to_finding).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::{DiffLine, DiffLineType, FileDiff, Hunk};
    use std::borrow::Cow;

    fn make_diff(path: &str, lines: &[&str]) -> FileDiff<'static> {
        let diff_lines: Vec<DiffLine<'static>> = lines
            .iter()
            .enumerate()
            .map(|(i, content)| DiffLine {
                line_type: DiffLineType::Added,
                content: Cow::Owned(content.to_string()),
                old_line_no: None,
                new_line_no: Some(i as u32 + 1),
            })
            .collect();

        FileDiff {
            old_path: "/dev/null".to_string(),
            new_path: path.to_string(),
            is_new: true,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: diff_lines.len() as u32,
                header: None,
                lines: diff_lines,
            }],
        }
    }

    #[tokio::test]
    async fn produces_findings_from_eval() {
        let rules = rules::default_rules();
        let diff = make_diff("malicious.js", &["const x = eval(userInput);"]);
        let findings = scan_for_threats(&[diff], &IndexMap::new(), &rules, None).await;

        assert!(!findings.is_empty(), "should detect eval() as a threat");
        assert_eq!(findings[0].agent, THREAT_SCANNER_AGENT);
        assert!(findings[0].title.contains("dangerous-api"));
    }

    #[tokio::test]
    async fn empty_diff_produces_no_findings() {
        let rules = rules::default_rules();
        let findings: Vec<Finding> = scan_for_threats(&[], &IndexMap::new(), &rules, None).await;
        assert!(findings.is_empty());
    }

    #[tokio::test]
    async fn finding_has_correct_agent_name() {
        let rules = rules::default_rules();
        let diff = make_diff("app.py", &["os.system('rm -rf /')"]);
        let findings = scan_for_threats(&[diff], &IndexMap::new(), &rules, None).await;

        for f in &findings {
            assert_eq!(f.agent, THREAT_SCANNER_AGENT);
        }
    }
}
