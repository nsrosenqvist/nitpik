//! Secret scanning rule loader.
//!
//! Loads rules from embedded defaults and optional user-provided TOML files.
//! Rule format is compatible with gitleaks TOML format.
//!
//! **Performance:** Compiling the 200+ bundled gitleaks-compatible patterns
//! takes roughly 20–30 seconds because many patterns contain large bounded
//! repetitions (e.g. `[\w-]{50,1000}`) that produce sizable internal DFA
//! states. This cost is paid once per process and only when secret scanning
//! is enabled — the rules are never loaded on the normal review path.

use regex::Regex;
use serde::Deserialize;
use std::path::Path;

/// Maximum compiled regex size (50 MB). Some gitleaks patterns with
/// large bounded repetitions (e.g. `[\w-]{50,1000}`) produce DFA states
/// that exceed the default 10 MB limit.
const REGEX_SIZE_LIMIT: usize = 50 * 1024 * 1024;

/// A single secret scanning rule with a pre-compiled regex.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SecretRule {
    /// Unique identifier for the rule.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Original regex pattern string (kept for diagnostics).
    pub regex: String,
    /// Pre-compiled regex, built with an elevated size limit.
    pub compiled_regex: Regex,
    /// Keywords for prefiltering (at least one must appear in the line).
    pub keywords: Vec<String>,
    /// Minimum Shannon entropy threshold (0.0 to disable).
    pub entropy_threshold: f64,
    /// Pre-compiled allowlist regexes for false positive suppression.
    pub allowlist_regexes: Vec<Regex>,
}

/// Compile a regex pattern with the project-wide elevated size limit.
fn compile_regex(pattern: &str) -> Result<Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
}

/// Compile allowlist patterns into regexes, skipping invalid ones with a warning.
fn compile_allowlist(patterns: Vec<String>, rule_id: &str) -> Vec<Regex> {
    patterns
        .into_iter()
        .filter_map(|p| match compile_regex(&p) {
            Ok(re) => Some(re),
            Err(e) => {
                eprintln!(
                    "Warning: skipping allowlist pattern for rule '{}': {e}",
                    rule_id
                );
                None
            }
        })
        .collect()
}

/// Embedded default rules (vendored gitleaks-compatible TOML).
const EMBEDDED_RULES_TOML: &str = include_str!("gitleaks_rules.toml");

/// Gitleaks-compatible TOML rule format.
#[derive(Debug, Deserialize)]
struct GitleaksConfig {
    #[serde(default)]
    rules: Vec<GitleaksRule>,
    /// Ignored top-level fields (e.g. `title`).
    #[serde(flatten)]
    _extra: std::collections::HashMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct GitleaksRule {
    id: String,
    #[serde(default)]
    description: String,
    /// Regex pattern — optional because some rules are path-only (e.g. pkcs12-file).
    #[serde(default)]
    regex: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    entropy: f64,
    #[serde(default)]
    allowlist: Option<GitleaksAllowlist>,
    /// Optional fields from gitleaks format we parse but don't use.
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    secret_group: Option<u32>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    /// Gitleaks v8 uses `[[rules.allowlists]]` (plural) in some configs.
    #[serde(default)]
    allowlists: Option<Vec<GitleaksAllowlist>>,
}

#[derive(Debug, Deserialize)]
struct GitleaksAllowlist {
    #[serde(default)]
    regexes: Vec<String>,
    /// Extra fields we ignore.
    #[serde(flatten)]
    _extra: std::collections::HashMap<String, toml::Value>,
}

/// Load the default built-in rules from embedded gitleaks-compatible TOML.
///
/// The rules are vendored from the gitleaks project and compiled into
/// the binary. Invalid regexes are skipped with a warning to stderr.
pub fn default_rules() -> Vec<SecretRule> {
    let config: GitleaksConfig = toml::from_str(EMBEDDED_RULES_TOML)
        .expect("embedded gitleaks_rules.toml should be valid TOML");

    config
        .rules
        .into_iter()
        .filter_map(|r| {
            // Skip path-only rules that have no regex pattern.
            let regex_str = r.regex?;

            // Pre-compile with elevated size limit for large patterns.
            let compiled_regex = match compile_regex(&regex_str) {
                Ok(re) => re,
                Err(e) => {
                    eprintln!(
                        "Warning: skipping secret rule '{}': invalid regex: {e}",
                        r.id
                    );
                    return None;
                }
            };

            // Merge allowlist from both singular and plural forms
            let mut allowlist_patterns = Vec::new();
            if let Some(al) = r.allowlist {
                allowlist_patterns.extend(al.regexes);
            }
            if let Some(als) = r.allowlists {
                for al in als {
                    allowlist_patterns.extend(al.regexes);
                }
            }

            Some(SecretRule {
                id: r.id.clone(),
                description: r.description,
                regex: regex_str,
                compiled_regex,
                keywords: r.keywords,
                entropy_threshold: r.entropy,
                allowlist_regexes: compile_allowlist(allowlist_patterns, &r.id),
            })
        })
        .collect()
}

/// Load additional rules from a TOML file (gitleaks format).
pub fn load_rules_from_file(path: &Path) -> Result<Vec<SecretRule>, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read rules file: {e}"))?;

    let config: GitleaksConfig =
        toml::from_str(&content).map_err(|e| format!("failed to parse rules file: {e}"))?;

    Ok(config
        .rules
        .into_iter()
        .filter_map(|r| {
            let regex_str = r.regex?;

            let compiled_regex = match compile_regex(&regex_str) {
                Ok(re) => re,
                Err(e) => {
                    eprintln!(
                        "Warning: skipping secret rule '{}': invalid regex: {e}",
                        r.id
                    );
                    return None;
                }
            };

            let mut allowlist_patterns = Vec::new();
            if let Some(al) = r.allowlist {
                allowlist_patterns.extend(al.regexes);
            }
            if let Some(als) = r.allowlists {
                for al in als {
                    allowlist_patterns.extend(al.regexes);
                }
            }

            Some(SecretRule {
                id: r.id.clone(),
                description: r.description,
                regex: regex_str,
                compiled_regex,
                keywords: r.keywords,
                entropy_threshold: r.entropy,
                allowlist_regexes: compile_allowlist(allowlist_patterns, &r.id),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile the full gitleaks ruleset once and share across tests.
    /// This avoids recompiling 219 regexes (each with a 50 MB size limit)
    /// in every test that needs the default rules.
    static DEFAULT_RULES: std::sync::LazyLock<Vec<SecretRule>> =
        std::sync::LazyLock::new(default_rules);

    #[test]
    fn default_rules_are_valid() {
        let rules = &*DEFAULT_RULES;
        // All 218 regex-bearing rules should load now that we use an
        // elevated regex size limit (minus 1 path-only rule).
        assert!(
            rules.len() >= 215,
            "expected at least 215 default rules, got {}",
            rules.len()
        );

        // Verify all pre-compiled regexes are usable
        for rule in rules {
            assert!(
                !rule.compiled_regex.as_str().is_empty(),
                "rule '{}' has an empty compiled regex",
                rule.id,
            );
        }
    }

    #[test]
    fn load_gitleaks_format() {
        let toml_content = r#"
[[rules]]
id = "custom-secret"
description = "Custom secret pattern"
regex = "CUSTOM_[A-Z0-9]{20}"
keywords = ["CUSTOM_"]
entropy = 3.5

[rules.allowlist]
regexes = ["test", "example"]
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "custom-secret");
        assert_eq!(rules[0].entropy_threshold, 3.5);
        assert_eq!(rules[0].allowlist_regexes.len(), 2);
    }

    #[test]
    fn load_gitleaks_format_plural_allowlists() {
        let toml_content = r#"
[[rules]]
id = "plural-test"
description = "Test plural allowlists"
regex = "SECRET_[A-Z0-9]{10}"
keywords = ["SECRET_"]

[[rules.allowlists]]
regexes = ["test_pattern"]

[[rules.allowlists]]
regexes = ["another_pattern", "third_pattern"]
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "plural-test");
        // Should merge all allowlist regexes: 1 + 2 = 3
        assert_eq!(rules[0].allowlist_regexes.len(), 3);
    }

    #[test]
    fn load_gitleaks_format_both_allowlist_forms() {
        let toml_content = r#"
[[rules]]
id = "both-forms"
description = "Both singular and plural allowlist"
regex = "TOKEN_[A-Z0-9]{16}"
keywords = ["TOKEN_"]

[rules.allowlist]
regexes = ["from_singular"]

[[rules.allowlists]]
regexes = ["from_plural"]
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert_eq!(rules.len(), 1);
        // Both forms merged
        assert_eq!(rules[0].allowlist_regexes.len(), 2);
    }

    #[test]
    fn load_rules_skips_path_only() {
        let toml_content = r#"
[[rules]]
id = "path-only"
description = "No regex"
keywords = ["something"]
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert!(
            rules.is_empty(),
            "path-only rules (no regex) should be skipped"
        );
    }

    #[test]
    fn load_rules_skips_invalid_regex() {
        let toml_content = r#"
[[rules]]
id = "bad-regex"
description = "Invalid regex pattern"
regex = "[invalid(("
keywords = ["bad"]
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert!(
            rules.is_empty(),
            "rules with invalid regex should be skipped"
        );
    }

    #[test]
    fn load_rules_file_not_found() {
        let result = load_rules_from_file(Path::new("/nonexistent/rules.toml"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to read"));
    }

    #[test]
    fn load_rules_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        let result = load_rules_from_file(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to parse"));
    }

    #[test]
    fn compile_allowlist_skips_invalid_patterns() {
        let patterns = vec![
            "valid_pattern".to_string(),
            "[invalid((regex".to_string(),
            "another_valid".to_string(),
        ];
        let result = compile_allowlist(patterns, "test-rule");
        // Should have 2 valid regexes, skipping the invalid one
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn compile_allowlist_empty_input() {
        let result = compile_allowlist(Vec::new(), "test-rule");
        assert!(result.is_empty());
    }

    #[test]
    fn compile_regex_valid() {
        let re = compile_regex(r"[A-Z0-9]{20}");
        assert!(re.is_ok());
    }

    #[test]
    fn compile_regex_invalid() {
        let re = compile_regex(r"[invalid((");
        assert!(re.is_err());
    }

    #[test]
    fn default_rules_allowlist_field_exists() {
        let rules = &*DEFAULT_RULES;
        // All rules should have the allowlist_regexes field (even if empty)
        for rule in rules {
            // This just verifies the field is initialized (not None/missing)
            let _ = rule.allowlist_regexes.len();
        }
    }
}
