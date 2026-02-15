//! Terminal renderer: styled flowing text grouped by file.
//!
//! Output style inspired by Semgrep/PHPStan — no tables.

use colored::Colorize;

use crate::models::finding::{Finding, Severity, Summary};
use crate::output::OutputRenderer;

/// Terminal output renderer with colored, flowing text.
pub struct TerminalRenderer;

impl OutputRenderer for TerminalRenderer {
    fn render(&self, findings: &[Finding]) -> String {
        if findings.is_empty() {
            return format!("{}", "  ✔ No issues found.\n".green());
        }

        let mut output = String::new();
        let mut sorted = findings.to_vec();
        sorted.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));

        let mut current_file = "";

        for finding in &sorted {
            // Group by file
            if finding.file != current_file {
                if !current_file.is_empty() {
                    output.push('\n');
                }
                current_file = &finding.file;
            }

            let (icon, severity_str) = match finding.severity {
                Severity::Error => (
                    "✖".red().bold().to_string(),
                    "error".red().bold().to_string(),
                ),
                Severity::Warning => (
                    "⚠".yellow().bold().to_string(),
                    "warning".yellow().bold().to_string(),
                ),
                Severity::Info => (
                    "ℹ".blue().bold().to_string(),
                    "info".blue().bold().to_string(),
                ),
            };

            let location = if let Some(end) = finding.end_line {
                format!("{}:{}-{}", finding.file, finding.line, end)
            } else {
                format!("{}:{}", finding.file, finding.line)
            };

            output.push_str(&format!(
                " {} {} in {}\n",
                icon,
                severity_str,
                location.bold()
            ));
            output.push_str(&format!(
                "   {} — {}\n",
                finding.title.bold(),
                finding.message
            ));

            if let Some(ref suggestion) = finding.suggestion {
                output.push_str(&format!("   {} {}\n", "→".cyan(), suggestion));
            }

            output.push('\n');
        }

        // Summary line
        let summary = Summary::from_findings(findings);
        output.push_str(&format!("{}\n", "───────────────────────────────────".dimmed()));
        output.push_str(&format!(
            " {} findings: {} {}, {} {}, {} {}\n",
            summary.total.to_string().bold(),
            summary.errors.to_string().red().bold(),
            if summary.errors == 1 { "error" } else { "errors" },
            summary.warnings.to_string().yellow().bold(),
            if summary.warnings == 1 { "warning" } else { "warnings" },
            summary.info.to_string().blue().bold(),
            if summary.info == 1 { "info" } else { "infos" },
        ));
        output.push_str(&format!(
            " {}\n",
            crate::constants::AI_DISCLOSURE.dimmed()
        ));

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_empty() {
        let renderer = TerminalRenderer;
        let output = renderer.render(&[]);
        assert!(output.contains("No issues found"));
    }

    #[test]
    fn render_findings() {
        let renderer = TerminalRenderer;
        let findings = vec![
            Finding {
                file: "src/main.rs".into(),
                line: 42,
                end_line: None,
                severity: Severity::Error,
                title: "Bug found".into(),
                message: "This is broken".into(),
                suggestion: Some("Fix it".into()),
                agent: "backend".into(),
            },
            Finding {
                file: "src/main.rs".into(),
                line: 67,
                end_line: None,
                severity: Severity::Warning,
                title: "Performance issue".into(),
                message: "Could be faster".into(),
                suggestion: None,
                agent: "backend".into(),
            },
        ];
        let output = renderer.render(&findings);
        // Check content is present (may be wrapped in ANSI color codes)
        assert!(output.contains("src/main.rs:42"));
        assert!(output.contains("Bug found"));
        assert!(output.contains("Fix it"));
        assert!(output.contains("findings"));
    }
}
