//! Secret scanner: detection, redaction, and rule loading.
//!
//! # Bounded Context: Secret Scanning
//!
//! Owns regex rule compilation, entropy analysis, secret detection,
//! and redaction. Runs before content reaches the LLM — produces
//! redacted text and security `Finding` values. Has no dependency
//! on providers or diff parsing.

pub mod entropy;
pub mod rules;
pub mod scanner;

use crate::models::finding::{Finding, Severity};

/// Scan content for secrets, redact them, and produce findings.
///
/// Returns (redacted_content, findings).
pub fn scan_and_redact(
    content: &str,
    file_path: &str,
    rules: &[rules::SecretRule],
    severity: Severity,
) -> (String, Vec<Finding>) {
    let matches = scanner::scan_content(content, rules);
    let mut findings = Vec::new();
    let mut redacted = content.to_string();

    // Process matches in reverse order to maintain string indices
    let mut sorted_matches = matches;
    sorted_matches.sort_by(|a, b| b.start.cmp(&a.start));

    for m in &sorted_matches {
        findings.push(Finding {
            file: file_path.to_string(),
            line: m.line_number,
            end_line: None,
            severity,
            title: format!("Potential secret detected: {}", m.rule_id),
            message: format!(
                "{}. The secret has been redacted before sending to the LLM.",
                m.rule_description.trim_end_matches('.')
            ),
            suggestion: Some("Remove the hardcoded secret and use environment variables or a secrets manager instead.".to_string()),
            agent: "secret-scanner".to_string(),
        });

        // Redact the matched secret
        let replacement = format!("[REDACTED:{}]", m.rule_id);
        redacted.replace_range(m.start..m.end, &replacement);
    }

    (redacted, findings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_rule(id: &str, pattern: &str, keywords: &[&str]) -> rules::SecretRule {
        rules::SecretRule {
            id: id.to_string(),
            description: format!("{id} secret"),
            regex: pattern.to_string(),
            compiled_regex: regex::Regex::new(pattern).unwrap(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            entropy_threshold: 0.0,
            allowlist_regexes: vec![],
        }
    }

    #[test]
    fn no_secrets_returns_original() {
        let rules = vec![make_test_rule(
            "api-key",
            r"API_KEY_[A-Z0-9]{10}",
            &["API_KEY"],
        )];
        let (redacted, findings) =
            scan_and_redact("fn main() {}", "test.rs", &rules, Severity::Warning);
        assert_eq!(redacted, "fn main() {}");
        assert!(findings.is_empty());
    }

    #[test]
    fn single_secret_redacted() {
        let rules = vec![make_test_rule(
            "api-key",
            r"SECRETKEY[A-Z0-9]{5}",
            &["SECRETKEY"],
        )];
        let content = "let key = \"SECRETKEYAB12X\";";
        let (redacted, findings) = scan_and_redact(content, "config.rs", &rules, Severity::Warning);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].agent, "secret-scanner");
        assert!(findings[0].title.contains("api-key"));
        assert!(redacted.contains("[REDACTED:api-key]"));
        assert!(!redacted.contains("SECRETKEYAB12X"));
    }

    #[test]
    fn multiple_secrets_redacted() {
        let rules = vec![make_test_rule("tok", r"TOK_[A-Z]{4}", &["TOK_"])];
        let content = "a = TOK_AAAA\nb = TOK_BBBB";
        let (redacted, findings) = scan_and_redact(content, "file.rs", &rules, Severity::Warning);

        assert_eq!(findings.len(), 2);
        assert!(!redacted.contains("TOK_AAAA"));
        assert!(!redacted.contains("TOK_BBBB"));
        assert_eq!(redacted.matches("[REDACTED:tok]").count(), 2);
    }

    #[test]
    fn finding_has_correct_fields() {
        let rules = vec![make_test_rule("pw", r"PASSWORD=[^\s]+", &["PASSWORD"])];
        let content = "line1\nPASSWORD=hunter2\nline3";
        let (_, findings) = scan_and_redact(content, "env.sh", &rules, Severity::Warning);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "env.sh");
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert!(findings[0].suggestion.is_some());
    }

    #[test]
    fn severity_is_configurable() {
        let rules = vec![make_test_rule("tok", r"TOK_[A-Z]{4}", &["TOK_"])];
        let content = "x = TOK_ABCD";

        let (_, error_findings) = scan_and_redact(content, "a.rs", &rules, Severity::Error);
        assert_eq!(error_findings[0].severity, Severity::Error);

        let (_, info_findings) = scan_and_redact(content, "a.rs", &rules, Severity::Info);
        assert_eq!(info_findings[0].severity, Severity::Info);
    }
}
