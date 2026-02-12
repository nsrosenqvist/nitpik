//! JSON output renderer.
//!
//! Outputs `{"findings": [...], "summary": {...}}` format.

use crate::models::finding::{Finding, Summary};
use crate::output::OutputRenderer;

/// JSON output renderer.
pub struct JsonRenderer;

impl OutputRenderer for JsonRenderer {
    fn render(&self, findings: &[Finding]) -> String {
        let summary = Summary::from_findings(findings);

        let output = serde_json::json!({
            "findings": findings,
            "summary": summary,
        });

        serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

    #[test]
    fn render_json() {
        let renderer = JsonRenderer;
        let findings = vec![Finding {
            file: "test.rs".into(),
            line: 1,
            end_line: None,
            severity: Severity::Warning,
            title: "Issue".into(),
            message: "Details".into(),
            suggestion: Some("Fix".into()),
            agent: "backend".into(),
        }];

        let output = renderer.render(&findings);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(parsed["findings"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["summary"]["total"], 1);
        assert_eq!(parsed["summary"]["warnings"], 1);
    }

    #[test]
    fn render_empty_json() {
        let renderer = JsonRenderer;
        let output = renderer.render(&[]);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["findings"].as_array().unwrap().len(), 0);
        assert_eq!(parsed["summary"]["total"], 0);
    }
}
