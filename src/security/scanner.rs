//! Secret scanning engine.
//!
//! Regex + keyword prefilter + entropy-based detection.

use super::entropy;
use super::rules::SecretRule;

/// A match found by the scanner.
#[derive(Debug)]
pub struct SecretMatch {
    pub rule_id: String,
    pub rule_description: String,
    pub line_number: u32,
    pub start: usize,
    pub end: usize,
}

/// Scan content against a set of secret rules.
pub fn scan_content(content: &str, rules: &[SecretRule]) -> Vec<SecretMatch> {
    let mut matches = Vec::new();

    for rule in rules {
        let regex = &rule.compiled_regex;

        for (line_idx, line) in content.lines().enumerate() {
            // Keyword prefilter: skip lines that don't contain any keywords
            if !rule.keywords.is_empty()
                && !rule
                    .keywords
                    .iter()
                    .any(|kw| line.to_lowercase().contains(&kw.to_lowercase()))
            {
                continue;
            }

            // Check allowlist
            if rule.allowlist_regexes.iter().any(|re| re.is_match(line)) {
                continue;
            }

            for m in regex.find_iter(line) {
                let matched_text = m.as_str();

                // Entropy check for generic patterns
                if rule.entropy_threshold > 0.0 {
                    let entropy_value = entropy::shannon_entropy(matched_text);
                    if entropy_value < rule.entropy_threshold {
                        continue;
                    }
                }

                // Calculate byte offset in the full content
                let line_start = content
                    .lines()
                    .take(line_idx)
                    .map(|l| l.len() + 1)
                    .sum::<usize>();

                matches.push(SecretMatch {
                    rule_id: rule.id.clone(),
                    rule_description: rule.description.clone(),
                    line_number: line_idx as u32 + 1,
                    start: line_start + m.start(),
                    end: line_start + m.end(),
                });
            }
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rule() -> SecretRule {
        let pattern = r#"(?i)api[_-]?key\s*[:=]\s*["']?[a-zA-Z0-9_]{20,}"#;
        SecretRule {
            id: "test-api-key".to_string(),
            description: "Test API key".to_string(),
            compiled_regex: regex::Regex::new(pattern).unwrap(),
            regex: pattern.to_string(),
            keywords: vec![
                "api_key".to_string(),
                "api-key".to_string(),
                "apikey".to_string(),
            ],
            entropy_threshold: 0.0,
            allowlist_regexes: vec![regex::Regex::new("example").unwrap()],
        }
    }

    #[test]
    fn detects_api_key() {
        let content = "config:\n  api_key: sk_live_abcdefghijklmnopqrst\n";
        let rules = vec![test_rule()];
        let matches = scan_content(content, &rules);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "test-api-key");
        assert_eq!(matches[0].line_number, 2);
    }

    #[test]
    fn skips_allowlisted() {
        let content = "# example api_key: sk_live_abcdefghijklmnopqrst\n";
        let rules = vec![test_rule()];
        let matches = scan_content(content, &rules);
        assert!(matches.is_empty());
    }

    #[test]
    fn skips_without_keyword() {
        let content = "nothing_here: sk_live_abcdefghijklmnopqrst\n";
        let rules = vec![test_rule()];
        let matches = scan_content(content, &rules);
        assert!(matches.is_empty());
    }
}
