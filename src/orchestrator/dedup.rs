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

use std::collections::{BTreeSet, HashMap};

use crate::models::finding::Finding;

/// Outcome of corroboration-aware deduplication.
pub struct DedupOutcome {
    /// The deduplicated findings, one representative per cluster.
    pub findings: Vec<Finding>,
    /// Map from each kept finding's [`fingerprint`] to the number of
    /// **distinct reviewer agents** that independently raised it. A
    /// value of 1 means a single lens flagged the issue; 2+ means
    /// cross-lens corroboration. Downstream passes (the critic) use
    /// this as a confidence signal: independently corroborated findings
    /// are far less likely to be false positives.
    pub corroboration: HashMap<String, u32>,
}

/// Stable identity for a deduplicated finding.
///
/// Built from file, line, and a normalized title — the fields that
/// stay fixed once a cluster's representative has been chosen. Lets a
/// finding be re-associated with its corroboration count after later
/// passes reorder or filter the set (e.g. diff-scope filtering), without
/// threading a parallel vector that drifts out of alignment.
pub fn fingerprint(f: &Finding) -> String {
    format!(
        "{}|{}|{}",
        f.file.to_lowercase(),
        f.line,
        f.title.to_lowercase()
    )
}

/// Deduplicate findings that are about the same issue.
///
/// Thin wrapper over [`deduplicate_with_corroboration`] for callers that
/// don't need the per-finding agreement count.
pub fn deduplicate(findings: Vec<Finding>) -> Vec<Finding> {
    deduplicate_with_corroboration(findings).findings
}

/// Deduplicate findings, preserving the cross-lens corroboration signal.
///
/// Two findings are considered duplicates if they have the same file,
/// overlapping line ranges, and at least one of the similarity signals
/// fires (shared evidence, title overlap, shared code symbol, or combined
/// text overlap).
///
/// Unlike a naive "keep the first" collapse, merging a cluster:
/// - **counts the distinct agents** that raised it (the corroboration
///   signal — multiple independent lenses agreeing is strong evidence the
///   issue is real),
/// - **unions the evidence** from every member so the representative
///   carries the strongest set of anchors (capped at
///   [`Finding::MAX_EVIDENCE`]), and
/// - **keeps the best representative** — highest severity, then most
///   evidence, then the fullest message — rather than whichever happened
///   to sort first.
///
/// Uses a file-keyed index so each finding is only compared against
/// other findings in the same file — O(n × k) where k is the per-file
/// count, instead of O(n²) over the entire result set.
pub fn deduplicate_with_corroboration(mut findings: Vec<Finding>) -> DedupOutcome {
    if findings.len() <= 1 {
        let corroboration = findings.iter().map(|f| (fingerprint(f), 1)).collect();
        return DedupOutcome {
            findings,
            corroboration,
        };
    }

    // Sort by file, then line for consistent dedup
    findings.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));

    /// One merged group of duplicate findings.
    struct Cluster {
        /// Best-phrased finding seen so far (see [`rep_rank`]).
        rep: Finding,
        /// Distinct agent names that raised a finding in this cluster.
        agents: BTreeSet<String>,
        /// Union of every member's evidence (case-insensitive, ordered).
        evidence: Vec<String>,
    }

    let mut clusters: Vec<Cluster> = Vec::new();
    // Index from file path → cluster indices for that file.
    let mut file_index: HashMap<String, Vec<usize>> = HashMap::new();

    for finding in findings {
        let hit = file_index.get(&finding.file).and_then(|indices| {
            indices.iter().copied().find(|&i| {
                let rep = &clusters[i].rep;
                lines_overlap(rep, &finding) && content_similar(rep, &finding)
            })
        });

        match hit {
            Some(i) => {
                let cluster = &mut clusters[i];
                cluster.agents.insert(finding.agent.clone());
                merge_evidence(&mut cluster.evidence, &finding.evidence);
                // Promote to the stronger representative phrasing. The
                // merged agent set and evidence union are unaffected.
                if rep_rank(&finding) > rep_rank(&cluster.rep) {
                    cluster.rep = finding;
                }
            }
            None => {
                let idx = clusters.len();
                file_index
                    .entry(finding.file.clone())
                    .or_default()
                    .push(idx);
                let mut agents = BTreeSet::new();
                agents.insert(finding.agent.clone());
                let evidence = finding.evidence.clone();
                clusters.push(Cluster {
                    rep: finding,
                    agents,
                    evidence,
                });
            }
        }
    }

    let mut result = Vec::with_capacity(clusters.len());
    let mut corroboration = HashMap::with_capacity(clusters.len());
    for mut cluster in clusters {
        cluster.evidence.truncate(Finding::MAX_EVIDENCE);
        cluster.rep.evidence = cluster.evidence;
        corroboration.insert(fingerprint(&cluster.rep), cluster.agents.len() as u32);
        result.push(cluster.rep);
    }

    DedupOutcome {
        findings: result,
        corroboration,
    }
}

/// Rank a finding as a cluster representative: more severe wins, then
/// the one carrying more evidence, then the fuller message. Compared as
/// a tuple so the ordering is total and stable.
fn rep_rank(f: &Finding) -> (u8, usize, usize) {
    (f.severity as u8, f.evidence.len(), f.message.len())
}

/// Append `extra` evidence entries not already present (case-insensitive).
fn merge_evidence(into: &mut Vec<String>, extra: &[String]) {
    for e in extra {
        if !into.iter().any(|x| x.eq_ignore_ascii_case(e)) {
            into.push(e.clone());
        }
    }
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
/// - At least one shared `evidence` entry (semantic primary signal).
/// - Title word overlap > 50%.
/// - At least one shared backtick-wrapped code symbol in title+message.
/// - Combined (title+message) word overlap > 50%.
///
/// Evidence is checked first because it's the strongest signal: when
/// both reviewers cite the same symbol or line, they are talking about
/// the same issue regardless of how they phrase the title.
fn content_similar(a: &Finding, b: &Finding) -> bool {
    // Signal 0: shared evidence (LLM-supplied semantic anchor).
    if has_shared_evidence(a, b) {
        return true;
    }

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

/// Compare evidence sets case-insensitively. Empty evidence on either
/// side never matches — absence of evidence is not a similarity signal.
fn has_shared_evidence(a: &Finding, b: &Finding) -> bool {
    if a.evidence.is_empty() || b.evidence.is_empty() {
        return false;
    }
    let a_norm: Vec<String> = a.evidence.iter().map(|s| s.to_lowercase()).collect();
    b.evidence
        .iter()
        .any(|e| a_norm.contains(&e.to_lowercase()))
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
            evidence: Vec::new(),
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
            evidence: Vec::new(),
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

    fn make_finding_with_evidence(
        file: &str,
        line: u32,
        title: &str,
        agent: &str,
        evidence: Vec<&str>,
    ) -> Finding {
        let mut f = make_finding(file, line, title, agent);
        f.evidence = evidence.into_iter().map(String::from).collect();
        f
    }

    #[test]
    fn shared_evidence_deduplicates_across_phrasings() {
        // Two reviewers, very different titles, but both citing the
        // same symbol as evidence — should collapse to one.
        let findings = vec![
            make_finding_with_evidence(
                "auth.rs",
                42,
                "Race condition risk in lock acquisition",
                "backend",
                vec!["acquire_session_lock"],
            ),
            make_finding_with_evidence(
                "auth.rs",
                42,
                "Insufficient mutual exclusion on session state",
                "security",
                vec!["acquire_session_lock", "session_mutex"],
            ),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn shared_evidence_match_is_case_insensitive() {
        let findings = vec![
            make_finding_with_evidence("x.rs", 1, "A different problem", "alpha", vec!["MyFunc"]),
            make_finding_with_evidence("x.rs", 1, "Yet another concern", "beta", vec!["myfunc"]),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 1);
    }

    // ── Corroboration ───────────────────────────────────────────────

    #[test]
    fn corroboration_counts_distinct_agents() {
        // Three reviewers flag the same issue (one twice); the cluster
        // should record three distinct lenses, not four findings.
        let findings = vec![
            make_finding("a.rs", 10, "SQL injection vulnerability", "security"),
            make_finding("a.rs", 10, "SQL injection vulnerability here", "backend"),
            make_finding("a.rs", 10, "SQL injection vulnerability found", "architect"),
            make_finding("a.rs", 10, "SQL injection vulnerability again", "security"),
        ];
        let out = deduplicate_with_corroboration(findings);
        assert_eq!(out.findings.len(), 1);
        let key = fingerprint(&out.findings[0]);
        // security, backend, architect — security's duplicate is not double-counted.
        assert_eq!(out.corroboration.get(&key), Some(&3));
    }

    #[test]
    fn lone_finding_has_corroboration_of_one() {
        let findings = vec![make_finding("a.rs", 1, "Solo issue", "backend")];
        let out = deduplicate_with_corroboration(findings);
        assert_eq!(
            out.corroboration.get(&fingerprint(&out.findings[0])),
            Some(&1)
        );
    }

    #[test]
    fn merge_keeps_highest_severity_representative() {
        // Same issue, two lenses — the error-severity phrasing must win
        // over the warning-severity one regardless of arrival order.
        let mut warn = make_finding("a.rs", 5, "Null deref in `parse`", "backend");
        warn.severity = Severity::Warning;
        let mut err = make_finding("a.rs", 5, "Null deref in `parse`", "security");
        err.severity = Severity::Error;

        let out = deduplicate_with_corroboration(vec![warn, err]);
        assert_eq!(out.findings.len(), 1);
        assert_eq!(out.findings[0].severity, Severity::Error);
    }

    #[test]
    fn merge_unions_evidence_across_lenses() {
        let a = make_finding_with_evidence(
            "a.rs",
            5,
            "Race on session lock",
            "backend",
            vec!["acquire_lock"],
        );
        let b = make_finding_with_evidence(
            "a.rs",
            5,
            "Race on session lock state",
            "security",
            vec!["acquire_lock", "session_mutex"],
        );
        let out = deduplicate_with_corroboration(vec![a, b]);
        assert_eq!(out.findings.len(), 1);
        let ev = &out.findings[0].evidence;
        assert!(ev.iter().any(|e| e == "acquire_lock"));
        assert!(ev.iter().any(|e| e == "session_mutex"));
        // No duplicate of the shared anchor.
        assert_eq!(ev.iter().filter(|e| *e == "acquire_lock").count(), 1);
    }

    #[test]
    fn merged_evidence_is_capped() {
        let a =
            make_finding_with_evidence("a.rs", 5, "Issue", "backend", vec!["e1", "e2", "e3", "e4"]);
        let b =
            make_finding_with_evidence("a.rs", 5, "Issue here", "security", vec!["e5", "e6", "e7"]);
        let out = deduplicate_with_corroboration(vec![a, b]);
        assert_eq!(out.findings.len(), 1);
        assert!(out.findings[0].evidence.len() <= Finding::MAX_EVIDENCE);
    }

    #[test]
    fn empty_evidence_does_not_deduplicate_unrelated_findings() {
        // Same file + line so the line-overlap check passes; only the
        // content_similar logic decides. With no evidence on either
        // side and no shared text, the findings stay separate.
        let findings = vec![
            make_finding("z.rs", 42, "Performance regression in hot path", "alpha"),
            make_finding("z.rs", 42, "Missing input validation", "beta"),
        ];
        let result = deduplicate(findings);
        assert_eq!(result.len(), 2);
    }
}
