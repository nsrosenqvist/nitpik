//! Checkstyle XML renderer.
//!
//! Outputs findings in the standard [Checkstyle XML format](https://checkstyle.sourceforge.io/),
//! which is widely supported by CI platforms and code quality tools.
//!
//! For Bitbucket Pipelines, pipe the output to a file and use the
//! [Checkstyle Code Insight Report](https://bitbucket.org/product/features/pipelines/integrations?search=checkstyle)
//! pipe to display findings as Code Insights annotations — no token required.
//!
//! ```yaml
//! script:
//!   - nitpik review --format checkstyle > checkstyle-report.xml
//! ```

use crate::models::finding::Finding;
use crate::output::OutputFormatter;
use crate::output::escape;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Checkstyle XML renderer.
///
/// Produces standard checkstyle XML output that can be consumed by
/// CI platform integrations, IDE plugins, and code quality tools.
pub struct CheckstyleFormatter;

impl OutputFormatter for CheckstyleFormatter {
    fn format(&self, findings: &[Finding]) -> String {
        // ~200 bytes per finding + ~100 bytes header/footer
        let mut output = String::with_capacity(100 + findings.len() * 200);
        output
            .push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<checkstyle version=\"4.3\">\n");

        // Group findings by file to produce one <file> element per path.
        let mut by_file: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
        for f in findings {
            by_file.entry(&f.file).or_default().push(f);
        }

        for (path, file_findings) in &by_file {
            let _ = writeln!(output, "  <file name=\"{}\">", escape::xml(path));
            for f in file_findings {
                let severity = f.severity.as_checkstyle_severity();

                let mut message = f.message.clone();
                if let Some(ref suggestion) = f.suggestion {
                    let _ = write!(message, "\n\nSuggestion: {suggestion}");
                }

                let _ = writeln!(
                    output,
                    "    <error line=\"{}\" severity=\"{}\" message=\"{}\" source=\"{}.{}\"/>",
                    f.line,
                    severity,
                    escape::xml(&message),
                    escape::xml(crate::constants::APP_NAME),
                    escape::xml(&f.agent),
                );
            }
            output.push_str("  </file>\n");
        }

        output.push_str("</checkstyle>\n");
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

    fn sample_findings() -> Vec<Finding> {
        vec![
            Finding {
                file: "src/main.rs".to_string(),
                line: 10,
                end_line: None,
                severity: Severity::Error,
                title: "Bug".to_string(),
                message: "A bug was found".to_string(),
                suggestion: Some("Fix the bug".to_string()),
                agent: "backend".to_string(),
            },
            Finding {
                file: "src/lib.rs".to_string(),
                line: 20,
                end_line: None,
                severity: Severity::Warning,
                title: "Style".to_string(),
                message: "Style issue".to_string(),
                suggestion: None,
                agent: "frontend".to_string(),
            },
            Finding {
                file: "src/main.rs".to_string(),
                line: 30,
                end_line: Some(35),
                severity: Severity::Info,
                title: "Note".to_string(),
                message: "Consider refactoring".to_string(),
                suggestion: None,
                agent: "architect".to_string(),
            },
        ]
    }

    #[test]
    fn render_produces_valid_xml_structure() {
        let output = CheckstyleFormatter.format(&sample_findings());
        assert!(output.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(output.contains("<checkstyle version=\"4.3\">"));
        assert!(output.ends_with("</checkstyle>\n"));
    }

    #[test]
    fn render_groups_by_file() {
        let output = CheckstyleFormatter.format(&sample_findings());
        // Two unique files: src/lib.rs and src/main.rs
        let file_count = output.matches("<file name=").count();
        assert_eq!(file_count, 2);
        // src/main.rs has two findings
        let main_errors =
            output.matches("line=\"10\"").count() + output.matches("line=\"30\"").count();
        assert_eq!(main_errors, 2);
    }

    #[test]
    fn render_maps_severity_correctly() {
        let output = CheckstyleFormatter.format(&sample_findings());
        assert!(output.contains("severity=\"error\""));
        assert!(output.contains("severity=\"warning\""));
        assert!(output.contains("severity=\"info\""));
    }

    #[test]
    fn render_includes_suggestion_in_message() {
        let output = CheckstyleFormatter.format(&sample_findings());
        assert!(output.contains("Suggestion: Fix the bug"));
    }

    #[test]
    fn render_includes_source_with_agent() {
        let output = CheckstyleFormatter.format(&sample_findings());
        assert!(output.contains("source=\"nitpik.backend\""));
        assert!(output.contains("source=\"nitpik.frontend\""));
        assert!(output.contains("source=\"nitpik.architect\""));
    }

    #[test]
    fn render_empty_findings() {
        let output = CheckstyleFormatter.format(&[]);
        assert!(output.starts_with("<?xml"));
        assert!(output.contains("<checkstyle"));
        assert!(output.contains("</checkstyle>"));
        assert!(!output.contains("<file"));
        assert!(!output.contains("<error"));
    }

    #[test]
    fn render_escapes_xml_special_chars() {
        let findings = vec![Finding {
            file: "src/foo&bar.rs".to_string(),
            line: 1,
            end_line: None,
            severity: Severity::Warning,
            title: "Test".to_string(),
            message: "Use <T> instead of \"raw\" types & 'stuff'".to_string(),
            suggestion: None,
            agent: "backend".to_string(),
        }];
        let output = CheckstyleFormatter.format(&findings);
        assert!(output.contains("name=\"src/foo&amp;bar.rs\""));
        assert!(output.contains("&lt;T&gt;"));
        assert!(output.contains("&quot;raw&quot;"));
        assert!(output.contains("&apos;stuff&apos;"));
    }

    #[test]
    fn render_files_in_sorted_order() {
        let output = CheckstyleFormatter.format(&sample_findings());
        let lib_pos = output.find("src/lib.rs").unwrap();
        let main_pos = output.find("src/main.rs").unwrap();
        assert!(lib_pos < main_pos, "Files should be sorted alphabetically");
    }
}
