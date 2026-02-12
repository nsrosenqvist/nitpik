//! Finding deduplication by file + line + similarity.

use crate::models::finding::Finding;

/// Deduplicate findings that are about the same issue.
///
/// Two findings are considered duplicates if they have the same file,
/// overlapping line ranges, and similar titles.
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
                && titles_similar(&existing.title, &finding.title)
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

/// Check if two titles are similar enough to be considered duplicates.
///
/// Uses a simple word overlap metric.
fn titles_similar(a: &str, b: &str) -> bool {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_words: Vec<&str> = a_lower.split_whitespace().collect();
    let b_words: Vec<&str> = b_lower.split_whitespace().collect();

    if a_words.is_empty() || b_words.is_empty() {
        return false;
    }

    let common = a_words
        .iter()
        .filter(|w| b_words.contains(w))
        .count();

    let max_len = a_words.len().max(b_words.len());
    let similarity = common as f64 / max_len as f64;

    similarity > 0.5
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
}
