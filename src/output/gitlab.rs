//! GitLab Code Quality report renderer.
//!
//! Outputs a JSON array conforming to the [CodeClimate spec](https://docs.gitlab.com/ee/ci/testing/code_quality.html)
//! that GitLab ingests via `artifacts:reports:codequality`.
//!
//! Users pipe the output to a file and declare it as a CI artifact:
//! ```yaml
//! artifacts:
//!   reports:
//!     codequality: gl-code-quality-report.json
//! ```

use crate::models::finding::{Finding, Severity};
use crate::output::OutputRenderer;

/// GitLab Code Quality renderer.
///
/// Emits a JSON array of CodeClimate-format objects suitable for use
/// as a `codequality` artifact in GitLab CI. Each finding is mapped to
/// a CodeClimate issue with a stable fingerprint derived from the
/// file path, line number, title, and message.
pub struct GitlabRenderer;

impl OutputRenderer for GitlabRenderer {
    fn render(&self, findings: &[Finding]) -> String {
        let entries: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                let severity = match f.severity {
                    Severity::Error => "critical",
                    Severity::Warning => "major",
                    Severity::Info => "minor",
                };

                let mut description = f.message.clone();
                if let Some(ref suggestion) = f.suggestion {
                    description.push_str(&format!("\n\nSuggestion: {suggestion}"));
                }

                let fingerprint = compute_fingerprint(f);

                let mut lines = serde_json::json!({ "begin": f.line });
                if let Some(end) = f.end_line {
                    lines["end"] = serde_json::json!(end);
                }

                serde_json::json!({
                    "description": description,
                    "check_name": f.title,
                    "fingerprint": fingerprint,
                    "severity": severity,
                    "location": {
                        "path": f.file,
                        "lines": lines,
                    },
                })
            })
            .collect();

        serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
    }
}

/// Compute a stable fingerprint for a finding.
///
/// Uses MD5 to produce a 32-char hex digest, matching the
/// [CodeClimate fingerprint convention](https://github.com/codeclimate/platform/blob/master/spec/analyzers/SPEC.md#fingerprints).
fn compute_fingerprint(f: &Finding) -> String {
    let input = format!("{}:{}:{}:{}", f.file, f.line, f.title, f.message);
    let digest = md5::compute(input.as_bytes());
    format!("{:x}", digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_single_finding() {
        let findings = vec![Finding {
            file: "src/auth.rs".into(),
            line: 42,
            end_line: Some(45),
            severity: Severity::Error,
            title: "SQL injection".into(),
            message: "User input interpolated into query.".into(),
            suggestion: Some("Use parameterized queries.".into()),
            agent: "security".into(),
        }];

        let output = GitlabRenderer.render(&findings);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();

        assert_eq!(parsed.len(), 1);
        let entry = &parsed[0];
        assert_eq!(entry["severity"], "critical");
        assert_eq!(entry["check_name"], "SQL injection");
        assert_eq!(entry["location"]["path"], "src/auth.rs");
        assert_eq!(entry["location"]["lines"]["begin"], 42);
        assert_eq!(entry["location"]["lines"]["end"], 45);
        assert!(entry["description"]
            .as_str()
            .unwrap()
            .contains("Suggestion: Use parameterized queries."));
        assert!(!entry["fingerprint"].as_str().unwrap().is_empty());
    }

    #[test]
    fn render_maps_severities() {
        let make = |severity| Finding {
            file: "f.rs".into(),
            line: 1,
            end_line: None,
            severity,
            title: "T".into(),
            message: "M".into(),
            suggestion: None,
            agent: "a".into(),
        };

        let findings = vec![
            make(Severity::Error),
            make(Severity::Warning),
            make(Severity::Info),
        ];
        let output = GitlabRenderer.render(&findings);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();

        assert_eq!(parsed[0]["severity"], "critical");
        assert_eq!(parsed[1]["severity"], "major");
        assert_eq!(parsed[2]["severity"], "minor");
    }

    #[test]
    fn render_omits_end_line_when_none() {
        let findings = vec![Finding {
            file: "f.rs".into(),
            line: 10,
            end_line: None,
            severity: Severity::Info,
            title: "T".into(),
            message: "M".into(),
            suggestion: None,
            agent: "a".into(),
        }];

        let output = GitlabRenderer.render(&findings);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert!(parsed[0]["location"]["lines"]["end"].is_null());
    }

    #[test]
    fn render_empty_findings() {
        let output = GitlabRenderer.render(&[]);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn fingerprint_is_stable() {
        let f = Finding {
            file: "a.rs".into(),
            line: 1,
            end_line: None,
            severity: Severity::Warning,
            title: "T".into(),
            message: "M".into(),
            suggestion: None,
            agent: "a".into(),
        };
        assert_eq!(compute_fingerprint(&f), compute_fingerprint(&f));
    }

    #[test]
    fn fingerprint_differs_for_different_findings() {
        let f1 = Finding {
            file: "a.rs".into(),
            line: 1,
            end_line: None,
            severity: Severity::Warning,
            title: "T".into(),
            message: "M1".into(),
            suggestion: None,
            agent: "a".into(),
        };
        let f2 = Finding {
            file: "a.rs".into(),
            line: 1,
            end_line: None,
            severity: Severity::Warning,
            title: "T".into(),
            message: "M2".into(),
            suggestion: None,
            agent: "a".into(),
        };
        assert_ne!(compute_fingerprint(&f1), compute_fingerprint(&f2));
    }

    #[test]
    fn no_suggestion_omits_suffix() {
        let findings = vec![Finding {
            file: "f.rs".into(),
            line: 1,
            end_line: None,
            severity: Severity::Info,
            title: "T".into(),
            message: "Just the message.".into(),
            suggestion: None,
            agent: "a".into(),
        }];

        let output = GitlabRenderer.render(&findings);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed[0]["description"], "Just the message.");
    }
}
