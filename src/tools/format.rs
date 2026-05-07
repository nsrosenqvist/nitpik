//! Shared formatting helpers for tool output.
//!
//! Tools return strings that are fed back into the LLM's context window.
//! These helpers keep tool output context-efficient and parseable:
//!
//! - [`format_with_line_numbers`] prefixes each line with a 5-digit
//!   1-based line number followed by ` | `, so the LLM can reference
//!   precise locations and we can ground findings in real lines.
//! - [`truncate_lines`] caps line-oriented output at a maximum number
//!   of lines and bytes, appending a `... showing N of M; <hint>`
//!   footer when truncated.
//!
//! All limits are conservative defaults — individual tools tighten or
//! relax them as appropriate.

/// Width of the line-number column rendered by [`format_with_line_numbers`].
///
/// 5 digits comfortably fits files up to 99,999 lines. Files larger than
/// that overflow gracefully (the column widens), preserving alignment for
/// the typical case without hard-failing on the edge.
pub const LINE_NUMBER_WIDTH: usize = 5;

/// Default soft cap on lines emitted by [`truncate_lines`] when callers
/// don't override.
pub const DEFAULT_MAX_LINES: usize = 2_000;

/// Default soft cap on bytes emitted by [`truncate_lines`] when callers
/// don't override.
pub const DEFAULT_MAX_BYTES: usize = 100 * 1024;

/// Outcome of a truncation decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncationOutcome {
    /// Number of lines actually included in the output.
    pub lines_included: usize,
    /// Total number of lines in the source.
    pub lines_total: usize,
    /// Whether the output was truncated by either limit.
    pub truncated: bool,
}

impl TruncationOutcome {
    /// `true` when the entire input fit within the limits. Test-only
    /// helper; production code reads `truncated` directly.
    #[cfg(test)]
    pub fn fits(&self) -> bool {
        !self.truncated
    }
}

/// Prefix each line of `content` with a right-aligned 1-based line number.
///
/// `start_line` is the line number assigned to the first line of `content`
/// — typically `1` for whole-file reads or the start of a requested range.
///
/// Format: `"{:>WIDTH} | {line}"`. If `content` ends with a newline, the
/// final empty line is *not* numbered (matches `cat -n` semantics).
pub fn format_with_line_numbers(content: &str, start_line: usize) -> String {
    if content.is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(content.len() + content.len() / 32);
    let lines = content.split('\n').collect::<Vec<_>>();

    // If the original content ended with '\n', split yields a trailing
    // empty element we don't want to number.
    let trailing_newline = content.ends_with('\n');
    let numbered = if trailing_newline {
        &lines[..lines.len().saturating_sub(1)]
    } else {
        &lines[..]
    };

    for (i, line) in numbered.iter().enumerate() {
        let n = start_line + i;
        out.push_str(&format!("{n:>LINE_NUMBER_WIDTH$} | {line}"));
        out.push('\n');
    }

    // Drop the final newline so callers can append a footer without
    // a blank line in between.
    if out.ends_with('\n') {
        out.pop();
    }

    out
}

/// Truncate a list of lines to fit `max_lines` and `max_bytes`.
///
/// Returns the joined output (newline-separated) and a [`TruncationOutcome`]
/// describing what happened. When truncation occurs, append a footer
/// returned by `footer` to the output.
///
/// The byte cap is checked progressively: as soon as the next line would
/// push the running total past `max_bytes`, truncation stops.
pub fn truncate_lines<F>(
    lines: &[String],
    max_lines: usize,
    max_bytes: usize,
    footer: F,
) -> (String, TruncationOutcome)
where
    F: FnOnce(&TruncationOutcome) -> String,
{
    let total = lines.len();
    let mut out = String::new();
    let mut included = 0;

    for line in lines.iter().take(max_lines) {
        // +1 for the newline we'll insert between lines.
        let projected = out.len() + line.len() + if included == 0 { 0 } else { 1 };
        if projected > max_bytes {
            break;
        }
        if included > 0 {
            out.push('\n');
        }
        out.push_str(line);
        included += 1;
    }

    let outcome = TruncationOutcome {
        lines_included: included,
        lines_total: total,
        truncated: included < total,
    };

    if outcome.truncated {
        let f = footer(&outcome);
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&f);
    }

    (out, outcome)
}

/// Format a byte count as a human-readable size string.
///
/// Used by tool result-summary lines so the audit log stays compact.
pub fn format_byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_lines_simple() {
        let out = format_with_line_numbers("a\nb\nc", 1);
        assert_eq!(out, "    1 | a\n    2 | b\n    3 | c");
    }

    #[test]
    fn format_lines_with_offset() {
        let out = format_with_line_numbers("first\nsecond", 42);
        assert_eq!(out, "   42 | first\n   43 | second");
    }

    #[test]
    fn format_lines_handles_trailing_newline() {
        let out = format_with_line_numbers("a\nb\n", 1);
        assert_eq!(out, "    1 | a\n    2 | b");
    }

    #[test]
    fn format_empty_input() {
        assert_eq!(format_with_line_numbers("", 1), "");
    }

    #[test]
    fn format_widens_for_large_numbers() {
        // 6-digit numbers right-align with our 5-wide minimum.
        let out = format_with_line_numbers("x", 100_000);
        assert_eq!(out, "100000 | x");
    }

    #[test]
    fn truncate_within_limits_is_lossless() {
        let lines = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let (out, outcome) = truncate_lines(&lines, 100, 1000, |_| "<truncated>".to_string());
        assert_eq!(out, "a\nb\nc");
        assert!(outcome.fits());
        assert_eq!(outcome.lines_included, 3);
        assert_eq!(outcome.lines_total, 3);
    }

    #[test]
    fn truncate_by_line_count() {
        let lines: Vec<String> = (0..10).map(|i| format!("line{i}")).collect();
        let (out, outcome) = truncate_lines(&lines, 3, 10_000, |o| {
            format!("... {} of {}", o.lines_included, o.lines_total)
        });
        assert!(outcome.truncated);
        assert_eq!(outcome.lines_included, 3);
        assert!(out.starts_with("line0\nline1\nline2"));
        assert!(out.contains("... 3 of 10"));
    }

    #[test]
    fn truncate_by_byte_count() {
        let big = "x".repeat(100);
        let lines: Vec<String> = (0..100).map(|_| big.clone()).collect();
        let (out, outcome) = truncate_lines(&lines, 1000, 250, |_| "<cut>".to_string());
        assert!(outcome.truncated);
        // We fit at most 2 lines of 100 chars + 1 newline = 201 bytes,
        // 3rd would push past 250.
        assert!(outcome.lines_included <= 2);
        assert!(out.ends_with("<cut>"));
    }

    #[test]
    fn truncate_empty_input() {
        let lines: Vec<String> = Vec::new();
        let (out, outcome) = truncate_lines(&lines, 100, 1000, |_| "<truncated>".to_string());
        assert!(out.is_empty());
        assert!(outcome.fits());
        assert_eq!(outcome.lines_total, 0);
    }

    #[test]
    fn format_byte_size_examples() {
        assert_eq!(format_byte_size(0), "0B");
        assert_eq!(format_byte_size(512), "512B");
        assert_eq!(format_byte_size(1024), "1.0KB");
        assert_eq!(format_byte_size(1024 * 1024), "1.0MB");
    }
}
