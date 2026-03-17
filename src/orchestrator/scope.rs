//! Diff-scope filtering for review findings.
//!
//! Ensures findings are restricted to lines that actually changed,
//! preventing false positives from unchanged context code.

use crate::models::diff::FileDiff;
use crate::models::finding::Finding;

/// Filter findings to only include those within diff hunk boundaries.
///
/// A finding is in scope if:
/// - Its file matches a file in the diffs, AND
/// - Its line number (or range) overlaps with at least one hunk's new-file range.
///
/// This prevents the LLM from reporting pre-existing issues in unchanged code
/// that was provided only as surrounding context.
pub fn filter_to_diff_scope(findings: Vec<Finding>, diffs: &[FileDiff<'_>]) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| finding_in_diff_scope(f, diffs))
        .collect()
}

/// Check whether a single finding falls within any diff hunk for its file.
fn finding_in_diff_scope(finding: &Finding, diffs: &[FileDiff<'_>]) -> bool {
    let Some(diff) = diffs.iter().find(|d| d.path() == finding.file) else {
        return false;
    };

    let finding_start = finding.line;
    let finding_end = finding.end_line.unwrap_or(finding.line);

    diff.hunks.iter().any(|hunk| {
        let hunk_start = hunk.new_start;
        let hunk_end = hunk
            .new_start
            .saturating_add(hunk.new_count)
            .saturating_sub(1);
        finding_start <= hunk_end && hunk_start <= finding_end
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::diff::{DiffLine, DiffLineType, Hunk};

    fn make_diff_with_hunk(path: &str, new_start: u32, new_count: u32) -> FileDiff<'static> {
        FileDiff {
            old_path: path.into(),
            new_path: path.into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: new_count,
                new_start,
                new_count,
                header: None,
                lines: (0..new_count)
                    .map(|i| DiffLine {
                        line_type: DiffLineType::Added,
                        content: std::borrow::Cow::Owned(format!("line {}", new_start + i)),
                        old_line_no: None,
                        new_line_no: Some(new_start + i),
                    })
                    .collect(),
            }],
        }
    }

    fn make_finding_at(file: &str, line: u32) -> Finding {
        Finding {
            file: file.into(),
            line,
            end_line: None,
            severity: crate::models::finding::Severity::Warning,
            title: "test".into(),
            message: "test".into(),
            suggestion: None,
            agent: "test".into(),
        }
    }

    #[test]
    fn finding_inside_hunk_kept() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("a.rs", 12)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_outside_hunk_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("a.rs", 50)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_on_hunk_boundary_start() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("a.rs", 10)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_on_hunk_boundary_end() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("a.rs", 14)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_just_before_hunk_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("a.rs", 9)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_just_after_hunk_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("a.rs", 15)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_wrong_file_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("b.rs", 12)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_with_range_overlapping_hunk() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let mut finding = make_finding_at("a.rs", 8);
        finding.end_line = Some(11);
        let result = filter_to_diff_scope(vec![finding], &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_with_range_not_overlapping_hunk() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let mut finding = make_finding_at("a.rs", 1);
        finding.end_line = Some(5);
        let result = filter_to_diff_scope(vec![finding], &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn mixed_findings_filtered_correctly() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![
            make_finding_at("a.rs", 10),
            make_finding_at("a.rs", 50),
            make_finding_at("a.rs", 14),
            make_finding_at("b.rs", 10),
        ];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].line, 10);
        assert_eq!(result[1].line, 14);
    }

    #[test]
    fn multiple_hunks_findings_in_second_hunk() {
        let mut diff = make_diff_with_hunk("a.rs", 10, 5);
        diff.hunks.push(Hunk {
            old_start: 50,
            old_count: 3,
            new_start: 50,
            new_count: 3,
            header: None,
            lines: vec![DiffLine {
                line_type: DiffLineType::Added,
                content: "line 50".into(),
                old_line_no: None,
                new_line_no: Some(50),
            }],
        });
        let findings = vec![
            make_finding_at("a.rs", 12),
            make_finding_at("a.rs", 30),
            make_finding_at("a.rs", 51),
        ];
        let result = filter_to_diff_scope(findings, &[diff]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].line, 12);
        assert_eq!(result[1].line, 51);
    }
}
