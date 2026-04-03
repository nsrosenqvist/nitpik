//! Threat scanning rule loader.
//!
//! Loads rules from embedded defaults and optional user-provided TOML files.
//! Rule format extends the gitleaks TOML convention with threat-specific
//! fields: `category`, `languages`, `scope`, and path-based allowlists.

use rayon::prelude::*;
use regex::Regex;
use serde::Deserialize;
use std::fmt;
use std::path::Path;

use crate::models::finding::Severity;

// ── Public types ────────────────────────────────────────────────────

/// Threat category for a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreatCategory {
    Obfuscation,
    DangerousApi,
    SupplyChain,
    Exfiltration,
    Backdoor,
}

impl fmt::Display for ThreatCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThreatCategory::Obfuscation => write!(f, "obfuscation"),
            ThreatCategory::DangerousApi => write!(f, "dangerous-api"),
            ThreatCategory::SupplyChain => write!(f, "supply-chain"),
            ThreatCategory::Exfiltration => write!(f, "exfiltration"),
            ThreatCategory::Backdoor => write!(f, "backdoor"),
        }
    }
}

impl std::str::FromStr for ThreatCategory {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "obfuscation" => Ok(ThreatCategory::Obfuscation),
            "dangerous-api" | "dangerousapi" => Ok(ThreatCategory::DangerousApi),
            "supply-chain" | "supplychain" => Ok(ThreatCategory::SupplyChain),
            "exfiltration" => Ok(ThreatCategory::Exfiltration),
            "backdoor" => Ok(ThreatCategory::Backdoor),
            _ => Err(format!("unknown threat category: {s}")),
        }
    }
}

/// Whether a rule matches against individual lines or full file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    /// Match against individual added lines.
    Line,
    /// Match against full baseline file content.
    File,
}

impl std::str::FromStr for RuleScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "line" => Ok(RuleScope::Line),
            "file" => Ok(RuleScope::File),
            _ => Err(format!(
                "unknown rule scope: {s} (expected 'line' or 'file')"
            )),
        }
    }
}

/// A single threat scanning rule with a pre-compiled regex.
#[derive(Debug, Clone)]
pub struct ThreatRule {
    /// Unique identifier for the rule.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Threat category.
    pub category: ThreatCategory,
    /// Finding severity when this rule matches.
    pub severity: Severity,
    /// Original regex pattern string (kept for diagnostics).
    pub regex: String,
    /// Pre-compiled regex.
    pub compiled_regex: Regex,
    /// Keywords for prefiltering (at least one must appear, case-insensitive).
    pub keywords: Vec<String>,
    /// File extension filter (empty means all languages).
    pub languages: Vec<String>,
    /// Whether to match per-line or per-file.
    pub scope: RuleScope,
    /// Minimum Shannon entropy threshold (0.0 to disable).
    pub entropy_threshold: f64,
    /// Minimum character length for matched text (0 to disable).
    pub min_match_length: usize,
    /// Pre-compiled allowlist regexes for false positive suppression.
    pub allowlist_regexes: Vec<Regex>,
    /// Path glob patterns for false positive suppression.
    pub allowlist_paths: Vec<String>,
}

/// Errors from threat rule loading.
#[derive(Debug)]
pub enum ThreatRuleError {
    IoError(std::io::Error),
    ParseError(String),
}

impl fmt::Display for ThreatRuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThreatRuleError::IoError(e) => write!(f, "failed to read rules file: {e}"),
            ThreatRuleError::ParseError(e) => write!(f, "failed to parse rules file: {e}"),
        }
    }
}

// ── Rule loading ────────────────────────────────────────────────────

/// Embedded default rules.
const EMBEDDED_RULES_TOML: &str = include_str!("threat_rules.toml");

/// Compile a regex pattern with the project-wide elevated size limit.
fn compile_regex(pattern: &str) -> Result<Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .size_limit(crate::constants::REGEX_SIZE_LIMIT)
        .build()
}

/// Compile allowlist patterns into regexes, skipping invalid ones with a warning.
fn compile_allowlist(patterns: Vec<String>, rule_id: &str) -> Vec<Regex> {
    patterns
        .into_iter()
        .filter_map(|p| match compile_regex(&p) {
            Ok(re) => Some(re),
            Err(e) => {
                eprintln!("Warning: skipping allowlist pattern for threat rule '{rule_id}': {e}");
                None
            }
        })
        .collect()
}

// ── TOML deserialization types ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ThreatRulesConfig {
    #[serde(default)]
    rules: Vec<RawThreatRule>,
}

#[derive(Debug, Deserialize)]
struct RawThreatRule {
    id: String,
    #[serde(default)]
    description: String,
    category: String,
    #[serde(default = "default_severity")]
    severity: String,
    #[serde(default)]
    regex: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default)]
    entropy: f64,
    #[serde(default)]
    min_match_length: usize,
    #[serde(default)]
    allowlist: Option<RawAllowlist>,
}

#[derive(Debug, Deserialize)]
struct RawAllowlist {
    #[serde(default)]
    regexes: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
}

fn default_severity() -> String {
    "warning".to_string()
}

fn default_scope() -> String {
    "line".to_string()
}

fn parse_severity(s: &str) -> Severity {
    match s.to_lowercase().as_str() {
        "info" => Severity::Info,
        "error" => Severity::Error,
        _ => Severity::Warning,
    }
}

/// Compile a single raw rule into a `ThreatRule`.
fn compile_threat_rule(r: RawThreatRule) -> Option<ThreatRule> {
    let regex_str = r.regex?;

    let compiled_regex = match compile_regex(&regex_str) {
        Ok(re) => re,
        Err(e) => {
            eprintln!(
                "Warning: skipping threat rule '{}': invalid regex: {e}",
                r.id
            );
            return None;
        }
    };

    let category: ThreatCategory = match r.category.parse() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: skipping threat rule '{}': {e}", r.id);
            return None;
        }
    };

    let scope: RuleScope = match r.scope.parse() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Warning: skipping threat rule '{}': {e}", r.id);
            return None;
        }
    };

    let (allowlist_regexes, allowlist_paths) = match r.allowlist {
        Some(al) => (compile_allowlist(al.regexes, &r.id), al.paths),
        None => (Vec::new(), Vec::new()),
    };

    Some(ThreatRule {
        id: r.id.clone(),
        description: r.description,
        category,
        severity: parse_severity(&r.severity),
        regex: regex_str,
        compiled_regex,
        keywords: r.keywords,
        languages: r.languages,
        scope,
        entropy_threshold: r.entropy,
        min_match_length: r.min_match_length,
        allowlist_regexes,
        allowlist_paths,
    })
}

/// Load the default built-in rules from embedded TOML.
pub fn default_rules() -> Vec<ThreatRule> {
    let config: ThreatRulesConfig = toml::from_str(EMBEDDED_RULES_TOML)
        .expect("embedded threat_rules.toml should be valid TOML");

    config
        .rules
        .into_par_iter()
        .filter_map(compile_threat_rule)
        .collect()
}

/// Load additional rules from a TOML file.
pub fn load_rules_from_file(path: &Path) -> Result<Vec<ThreatRule>, ThreatRuleError> {
    let content = std::fs::read_to_string(path).map_err(ThreatRuleError::IoError)?;

    let config: ThreatRulesConfig =
        toml::from_str(&content).map_err(|e| ThreatRuleError::ParseError(e.to_string()))?;

    Ok(config
        .rules
        .into_par_iter()
        .filter_map(compile_threat_rule)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile the full ruleset once and share across tests.
    static DEFAULT_RULES: std::sync::LazyLock<Vec<ThreatRule>> =
        std::sync::LazyLock::new(default_rules);

    #[test]
    fn default_rules_are_valid() {
        let rules = &*DEFAULT_RULES;
        assert!(
            rules.len() >= 20,
            "expected at least 20 default threat rules, got {}",
            rules.len()
        );

        for rule in rules {
            assert!(
                !rule.compiled_regex.as_str().is_empty(),
                "rule '{}' has an empty compiled regex",
                rule.id,
            );
        }
    }

    #[test]
    fn default_rules_have_valid_categories() {
        for rule in &*DEFAULT_RULES {
            // Display roundtrip check
            let displayed = rule.category.to_string();
            let parsed: ThreatCategory = displayed.parse().unwrap_or_else(|e| {
                panic!(
                    "rule '{}' category '{}' failed roundtrip: {e}",
                    rule.id, displayed
                )
            });
            assert_eq!(parsed, rule.category);
        }
    }

    #[test]
    fn load_custom_rules() {
        let toml_content = r#"
[[rules]]
id = "custom-eval"
description = "Custom eval detection"
category = "dangerous-api"
severity = "error"
regex = '''eval\s*\('''
keywords = ["eval"]
languages = ["js"]
scope = "line"

[rules.allowlist]
regexes = ["test"]
paths = ["**/test/**"]
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "custom-eval");
        assert_eq!(rules[0].category, ThreatCategory::DangerousApi);
        assert_eq!(rules[0].severity, Severity::Error);
        assert_eq!(rules[0].languages, vec!["js"]);
        assert_eq!(rules[0].scope, RuleScope::Line);
        assert_eq!(rules[0].allowlist_regexes.len(), 1);
        assert_eq!(rules[0].allowlist_paths, vec!["**/test/**"]);
    }

    #[test]
    fn invalid_category_skipped() {
        let toml_content = r#"
[[rules]]
id = "bad-category"
description = "Bad"
category = "nonexistent"
regex = "test"
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn invalid_regex_skipped() {
        let toml_content = r#"
[[rules]]
id = "bad-regex"
description = "Bad regex"
category = "obfuscation"
regex = "[invalid("
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn missing_regex_skipped() {
        let toml_content = r#"
[[rules]]
id = "no-regex"
description = "No regex field"
category = "backdoor"
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, toml_content).unwrap();

        let rules = load_rules_from_file(&path).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn category_parsing() {
        assert_eq!(
            "obfuscation".parse::<ThreatCategory>().unwrap(),
            ThreatCategory::Obfuscation
        );
        assert_eq!(
            "dangerous-api".parse::<ThreatCategory>().unwrap(),
            ThreatCategory::DangerousApi
        );
        assert_eq!(
            "supply-chain".parse::<ThreatCategory>().unwrap(),
            ThreatCategory::SupplyChain
        );
        assert_eq!(
            "exfiltration".parse::<ThreatCategory>().unwrap(),
            ThreatCategory::Exfiltration
        );
        assert_eq!(
            "backdoor".parse::<ThreatCategory>().unwrap(),
            ThreatCategory::Backdoor
        );
        assert!("unknown".parse::<ThreatCategory>().is_err());
    }

    #[test]
    fn scope_parsing() {
        assert_eq!("line".parse::<RuleScope>().unwrap(), RuleScope::Line);
        assert_eq!("file".parse::<RuleScope>().unwrap(), RuleScope::File);
        assert!("block".parse::<RuleScope>().is_err());
    }
}
