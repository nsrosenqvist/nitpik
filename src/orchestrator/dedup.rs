//! Finding deduplication by file + line + similarity.
//!
//! Two findings are considered duplicates when they share the same file,
//! overlapping line ranges, and *any* of three similarity signals:
//!
//! 1. **Title word overlap >50%** — the original heuristic.
//! 2. **Shared code symbol** — backtick-wrapped identifiers (e.g.
//!    `` `pickle.loads` ``) that appear in both findings' title+message.
//! 3. **Combined text overlap >50%** — word overlap computed over the
//!    concatenation of title + message, giving a larger corpus that
//!    naturally includes shared variable/function names.

use crate::models::finding::Finding;

/// Deduplicate findings that are about the same issue.
///
/// Two findings are considered duplicates if they have the same file,
/// overlapping line ranges, and at least one of the similarity signals
/// fires (title overlap, shared code symbol, or combined text overlap).
pub fn deduplicate(mut findings: Vec<Finding>) -> Vec<Finding> {
    if findings.len() <= 1 {
        return findings;
    }

    // Sort by file, then line for consistent dedup
    findings.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));

    let mut result: Vec<Finding> = Vec::new();

    for finding in findings {
        let is_dup = result.iter().any(|existing| {
            existing.file == finding.file
                && lines_overlap(existing, &finding)
                && content_similar(existing, &finding)
        });

        if !is_dup {
            result.push(finding);
        }
    }

    result
}

/// Check if two findings have overlapping line ranges.
fn lines_overlap(a: &Finding, b: &Finding) -> bool {
    let a_end = a.end_line.unwrap_or(a.line);
    let b_end = b.end_line.unwrap_or(b.line);

    a.line <= b_end && b.line <= a_end
}

/// Check if two findings describe the same issue using multiple signals.
///
/// Returns `true` if *any* of the following match:
/// - Title word overlap > 50%
/// - At least one shared backtick-wrapped code symbol in title+message
/// - Combined (title+message) word overlap > 50%
fn content_similar(a: &Finding, b: &Finding) -> bool {
    // Signal 1: title word overlap (original heuristic)
    if word_overlap(&a.title, &b.title) > 0.5 {
        return true;
    }

    // Signal 2: shared code symbols extracted from backtick spans
    let a_text = combined_text(a);
    let b_text = combined_text(b);
    if has_shared_code_symbol(&a_text, &b_text) {
        return true;
    }

    // Signal 3: combined (title + message) word overlap
    if word_overlap(&a_text, &b_text) > 0.5 {
        return true;
    }

    false
}

/// Concatenate title and message for broader similarity comparison.
fn combined_text(f: &Finding) -> String {
    format!("{} {}", f.title, f.message)
}

/// Compute word overlap ratio between two strings.
///
/// Returns a value in `[0.0, 1.0]` representing the fraction of words
/// (from the longer string) that appear in both.
fn word_overlap(a: &str, b: &str) -> f64 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_words: Vec<&str> = a_lower.split_whitespace().collect();
    let b_words: Vec<&str> = b_lower.split_whitespace().collect();

    if a_words.is_empty() || b_words.is_empty() {
        return 0.0;
    }

    let common = a_words.iter().filter(|w| b_words.contains(w)).count();
    let max_len = a_words.len().max(b_words.len());

    common as f64 / max_len as f64
}

/// Extract backtick-wrapped code symbols and check for overlap.
///
/// Symbols shorter than 3 characters are ignored to avoid false
/// positives on single-letter variables or empty backtick pairs.
fn has_shared_code_symbol(a: &str, b: &str) -> bool {
    let a_symbols = extract_code_symbols(a);
    if a_symbols.is_empty() {
        return false;
    }
    let b_symbols = extract_code_symbols(b);
    a_symbols.iter().any(|s| b_symbols.contains(s))
}

/// Extract backtick-delimited code spans, normalized to lowercase with
/// trailing punctuation stripped (e.g. `` `pickle.loads()` `` → `pickle.loads`).
fn extract_code_symbols(text: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find('`') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('`') {
            let raw = &rest[..end];
            let normalized = raw
                .trim()
                .trim_end_matches(['(', ')', ';', ','])
                .to_lowercase();
            if normalized.len() >= 3 {
                symbols.push(normalized);
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }

    symbols
}

/// Legacy compatibility alias — still used in tests.
#[allow(dead_code)]
fn titles_similar(a: &str, b: &str) -> bool {
    word_overlap(a, b) > 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

    fn make_finding(file: &str, line: u32, title: &str, agent: &str) -> Finding {
        Finding {
            file: file.into(),
            line,
            end_line: None,
            severity: Severity::Warning,
            title: title.into(),
            message: "msg".into(),
            suggestion: None,
            agent: agent.into(),
        }
    }

    fn make_finding_full(
        file: &str,
        line: u32,
        title: &str,
        message: &str,
        agent: &str,
    ) -> Finding {
        Finding {
            file: file.into(),
            line,
            end_line: None,
            severity: Severity::Warning,
            title: title.into(),
            message: message.into(),
            suggestion: None,
            agent: agent.into(),
        }
    }

    // ── Existing tests ──────────────────────────────────────────────

    #[test]
    fn no_duplicates() {
        let findings = vec![
            make_finding("a.rs", 1, "Issue A", "agent1"),
            make_finding("b.rs", 2, "Issue B", "agent1"),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn exact_duplicate_removed() {
        let findings = vec![
            make_finding("a.rs", 10, "SQL injection vulnerability", "security"),
            make_finding("a.rs", 10, "SQL injection vulnerability found", "backend"),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn different_files_not_deduped() {
        let findings = vec![
            make_finding("a.rs", 10, "Same issue", "agent1"),
            make_finding("b.rs", 10, "Same issue", "agent1"),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn different_titles_not_deduped() {
        let findings = vec![
            make_finding("a.rs", 10, "Issue A", "agent1"),
            make_finding("a.rs", 10, "Completely different B", "agent1"),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 2);
    }

    // ── Code symbol dedup ───────────────────────────────────────────

    #[test]
    fn shared_code_symbol_deduplicates() {
        // Different titles but same backtick-wrapped symbol
        let findings = vec![
            make_finding_full(
                "main.py",
                50,
                "Deserializing untrusted data with `pickle.loads()` is dangerous",
                "Can lead to arbitrary code execution.",
                "security",
            ),
            make_finding_full(
                "main.py",
                50,
                "Using `pickle.loads` on untrusted input compromises server integrity",
                "Avoid pickle for untrusted data.",
                "backend",
            ),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn shared_code_symbol_yaml_load() {
        let findings = vec![
            make_finding_full(
                "main.py",
                60,
                "Using `yaml.load()` without safe loader",
                "Can lead to arbitrary code execution.",
                "security",
            ),
            make_finding_full(
                "main.py",
                60,
                "Unsafe YAML deserialization via `yaml.load`",
                "Allows attackers to modify config or execute commands.",
                "backend",
            ),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn different_symbols_not_deduped() {
        let findings = vec![
            make_finding_full(
                "main.py",
                50,
                "Unsafe deserialization via `pickle.loads()`",
                "Deserializing untrusted pickle data can lead to arbitrary code execution through crafted payloads.",
                "security",
            ),
            make_finding_full(
                "main.py",
                50,
                "Missing safe loader for `yaml.load()`",
                "Loading YAML without a safe loader allows tag-based object instantiation and remote code execution.",
                "backend",
            ),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 2);
    }

    // ── Combined text overlap dedup ─────────────────────────────────

    #[test]
    fn combined_text_deduplicates_paraphrased() {
        // Titles differ but messages share the same variable names and vulnerability description
        let findings = vec![
            make_finding_full(
                "main.py",
                70,
                "Command injection via user input in os.popen",
                "The cmd variable from user input request.json is executed via os.popen without sanitization, leading to command injection vulnerability.",
                "security",
            ),
            make_finding_full(
                "main.py",
                70,
                "User input executed via os.popen without sanitization",
                "The cmd variable derived from user input request.json is passed to os.popen, leading to a command injection vulnerability on the server.",
                "backend",
            ),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 1);
    }

    // ── extract_code_symbols ────────────────────────────────────────

    #[test]
    fn extract_symbols_basic() {
        let symbols = extract_code_symbols("Using `pickle.loads()` is bad");
        assert_eq!(symbols, vec!["pickle.loads"]);
    }

    #[test]
    fn extract_symbols_multiple() {
        let symbols = extract_code_symbols("Use `yaml.safe_load()` instead of `yaml.load()`");
        assert_eq!(symbols, vec!["yaml.safe_load", "yaml.load"]);
    }

    #[test]
    fn extract_symbols_ignores_short() {
        let symbols = extract_code_symbols("Variable `x` is unused");
        assert!(symbols.is_empty());
    }

    #[test]
    fn extract_symbols_strips_parens() {
        let symbols = extract_code_symbols("Call `os.popen()` is dangerous");
        assert_eq!(symbols, vec!["os.popen"]);
    }

    // ── word_overlap ────────────────────────────────────────────────

    #[test]
    fn word_overlap_identical() {
        assert!(word_overlap("SQL injection vulnerability", "SQL injection vulnerability") > 0.99);
    }

    #[test]
    fn word_overlap_empty() {
        assert_eq!(word_overlap("", "something"), 0.0);
        assert_eq!(word_overlap("something", ""), 0.0);
    }
}
