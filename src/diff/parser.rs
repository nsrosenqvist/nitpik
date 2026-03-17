//! Unified diff format parser.
//!
//! Parses the output of `git diff` (unified format) into `Vec<FileDiff>`.
//!
//! Uses `Cow::Borrowed` so that parsed `DiffLine` content borrows
//! directly from the input string, avoiding per-line allocations.

use std::borrow::Cow;

use crate::models::diff::{DiffLine, DiffLineType, FileDiff, Hunk};

/// States for the unified diff parser state machine.
enum ParserState {
    /// Scanning for the next `diff --git` line.
    SeekingDiff,
    /// Parsing extended headers (file mode, rename, index, binary) after a
    /// `diff --git` line has been consumed.
    Header,
    /// Inside a hunk body, parsing `+`, `-`, ` `, and `\` lines.
    HunkBody,
}

/// Parse a unified diff string into a list of file diffs.
///
/// The returned `FileDiff` values borrow line content from `input`.
pub fn parse_unified_diff<'a>(input: &'a str) -> Vec<FileDiff<'a>> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = input.lines().peekable();

    let mut state = ParserState::SeekingDiff;

    // Per-file accumulators — initialized when we enter Header state.
    let mut old_path = String::new();
    let mut new_path = String::new();
    let mut is_new = false;
    let mut is_deleted = false;
    let mut is_rename = false;
    let mut is_binary = false;
    let mut hunks: Vec<Hunk> = Vec::new();

    // Per-hunk accumulators — initialized when we enter HunkBody state.
    let mut hunk_old_start: u32 = 0;
    let mut hunk_old_count: u32 = 0;
    let mut hunk_new_start: u32 = 0;
    let mut hunk_new_count: u32 = 0;
    let mut hunk_header: Option<String> = None;
    let mut hunk_lines: Vec<DiffLine<'a>> = Vec::new();
    let mut old_line: u32 = 0;
    let mut new_line: u32 = 0;

    while let Some(&line) = lines.peek() {
        match state {
            ParserState::SeekingDiff => {
                if line.starts_with("diff --git ") {
                    let (op, np) = parse_diff_header(line);
                    old_path = op;
                    new_path = np;
                    is_new = false;
                    is_deleted = false;
                    is_rename = false;
                    is_binary = false;
                    hunks = Vec::new();
                    state = ParserState::Header;
                }
                lines.next();
            }

            ParserState::Header => {
                if line.starts_with("diff --git ") {
                    // New file diff — flush current file and stay in Header
                    // via transition to SeekingDiff (which will immediately
                    // re-enter Header on the next iteration).
                    files.push(FileDiff {
                        old_path: std::mem::take(&mut old_path),
                        new_path: std::mem::take(&mut new_path),
                        is_new,
                        is_deleted,
                        is_rename,
                        is_binary,
                        hunks: std::mem::take(&mut hunks),
                    });
                    state = ParserState::SeekingDiff;
                    // Don't consume the line — SeekingDiff will pick it up.
                } else if line.starts_with("new file mode") {
                    is_new = true;
                    lines.next();
                } else if line.starts_with("deleted file mode") {
                    is_deleted = true;
                    lines.next();
                } else if line.starts_with("rename from") || line.starts_with("rename to") {
                    is_rename = true;
                    lines.next();
                } else if line.starts_with("similarity index")
                    || line.starts_with("dissimilarity index")
                    || line.starts_with("index ")
                {
                    lines.next();
                } else if line.contains("Binary files") {
                    is_binary = true;
                    lines.next();
                } else if line.starts_with("---") || line.starts_with("+++") {
                    lines.next();
                } else if line.starts_with("@@") {
                    // Transition to hunk body.
                    if let Some((os, oc, ns, nc, hh)) = parse_hunk_header(line) {
                        hunk_old_start = os;
                        hunk_old_count = oc;
                        hunk_new_start = ns;
                        hunk_new_count = nc;
                        hunk_header = hh;
                        hunk_lines = Vec::new();
                        old_line = os;
                        new_line = ns;
                        state = ParserState::HunkBody;
                    }
                    lines.next();
                } else {
                    lines.next();
                }
            }

            ParserState::HunkBody => {
                if line.starts_with("diff --git ") {
                    // Flush current hunk, flush current file.
                    hunks.push(Hunk {
                        old_start: hunk_old_start,
                        old_count: hunk_old_count,
                        new_start: hunk_new_start,
                        new_count: hunk_new_count,
                        header: hunk_header.take(),
                        lines: std::mem::take(&mut hunk_lines),
                    });
                    files.push(FileDiff {
                        old_path: std::mem::take(&mut old_path),
                        new_path: std::mem::take(&mut new_path),
                        is_new,
                        is_deleted,
                        is_rename,
                        is_binary,
                        hunks: std::mem::take(&mut hunks),
                    });
                    state = ParserState::SeekingDiff;
                    // Don't consume — SeekingDiff will pick it up.
                } else if line.starts_with("@@") {
                    // Flush current hunk, start a new one.
                    hunks.push(Hunk {
                        old_start: hunk_old_start,
                        old_count: hunk_old_count,
                        new_start: hunk_new_start,
                        new_count: hunk_new_count,
                        header: hunk_header.take(),
                        lines: std::mem::take(&mut hunk_lines),
                    });
                    if let Some((os, oc, ns, nc, hh)) = parse_hunk_header(line) {
                        hunk_old_start = os;
                        hunk_old_count = oc;
                        hunk_new_start = ns;
                        hunk_new_count = nc;
                        hunk_header = hh;
                        hunk_lines = Vec::new();
                        old_line = os;
                        new_line = ns;
                    } else {
                        // Malformed hunk header — fall back to header state.
                        state = ParserState::Header;
                    }
                    lines.next();
                } else if let Some(content) = line.strip_prefix('+') {
                    hunk_lines.push(DiffLine {
                        line_type: DiffLineType::Added,
                        content: Cow::Borrowed(content),
                        old_line_no: None,
                        new_line_no: Some(new_line),
                    });
                    new_line += 1;
                    lines.next();
                } else if let Some(content) = line.strip_prefix('-') {
                    hunk_lines.push(DiffLine {
                        line_type: DiffLineType::Removed,
                        content: Cow::Borrowed(content),
                        old_line_no: Some(old_line),
                        new_line_no: None,
                    });
                    old_line += 1;
                    lines.next();
                } else if line.starts_with(' ') || line.is_empty() {
                    let content = if line.is_empty() {
                        Cow::Borrowed("")
                    } else {
                        Cow::Borrowed(&line[1..])
                    };
                    hunk_lines.push(DiffLine {
                        line_type: DiffLineType::Context,
                        content,
                        old_line_no: Some(old_line),
                        new_line_no: Some(new_line),
                    });
                    old_line += 1;
                    new_line += 1;
                    lines.next();
                } else if line.starts_with('\\') {
                    // "\ No newline at end of file" — skip
                    lines.next();
                } else {
                    // Unknown line format — flush hunk and return to header parsing.
                    hunks.push(Hunk {
                        old_start: hunk_old_start,
                        old_count: hunk_old_count,
                        new_start: hunk_new_start,
                        new_count: hunk_new_count,
                        header: hunk_header.take(),
                        lines: std::mem::take(&mut hunk_lines),
                    });
                    state = ParserState::Header;
                    lines.next();
                }
            }
        }
    }

    // Flush any in-progress hunk and file.
    if matches!(state, ParserState::HunkBody) {
        hunks.push(Hunk {
            old_start: hunk_old_start,
            old_count: hunk_old_count,
            new_start: hunk_new_start,
            new_count: hunk_new_count,
            header: hunk_header.take(),
            lines: std::mem::take(&mut hunk_lines),
        });
    }
    if matches!(state, ParserState::Header | ParserState::HunkBody) {
        files.push(FileDiff {
            old_path,
            new_path,
            is_new,
            is_deleted,
            is_rename,
            is_binary,
            hunks,
        });
    }

    files
}

/// Parse the "diff --git a/path b/path" header line.
fn parse_diff_header(line: &str) -> (String, String) {
    let rest = line.strip_prefix("diff --git ").unwrap_or(line);

    // Handle paths with spaces by finding the second prefix separator
    // Paths are prefixed with a/ and b/ (default), or c/w/i/o/ when
    // git's diff.mnemonicPrefix is enabled.
    if let Some(b_idx) = find_second_prefix(rest) {
        let a_part = &rest[..b_idx];
        let b_part = &rest[b_idx + 1..]; // skip the space

        let old_path = strip_diff_prefix(a_part).to_string();
        let new_path = strip_diff_prefix(b_part).to_string();
        (old_path, new_path)
    } else {
        // Fallback: split on space
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let old_path = strip_diff_prefix(parts.first().unwrap_or(&"")).to_string();
        let new_path = strip_diff_prefix(parts.get(1).unwrap_or(&"")).to_string();
        (old_path, new_path)
    }
}

/// Strip a single-character git diff prefix (`a/`, `b/`, `c/`, `w/`, `i/`, `o/`).
///
/// These prefixes appear in `diff --git` headers:
/// - `a/` and `b/` are the defaults.
/// - `c/` (commit), `w/` (working tree), `i/` (index), `o/` (object)
///   are used when `diff.mnemonicPrefix` is enabled.
fn strip_diff_prefix(path: &str) -> &str {
    if path.len() >= 2 {
        let bytes = path.as_bytes();
        if bytes[1] == b'/' && matches!(bytes[0], b'a' | b'b' | b'c' | b'w' | b'i' | b'o') {
            return &path[2..];
        }
    }
    path
}

/// Find the position of the second path prefix separator in a diff header.
///
/// Looks for ` X/` where X is any known single-letter prefix (`a`..`o`).
fn find_second_prefix(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    for i in 1..bytes.len().saturating_sub(1) {
        if bytes[i] == b' '
            && bytes.get(i + 2) == Some(&b'/')
            && matches!(
                bytes.get(i + 1),
                Some(b'a' | b'b' | b'c' | b'w' | b'i' | b'o')
            )
        {
            return Some(i);
        }
    }
    None
}

/// Parse a `@@ -old_start,old_count +new_start,new_count @@ header` line.
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, Option<String>)> {
    let line = line.strip_prefix("@@ ")?;
    let end = line.find(" @@")?;
    let range_part = &line[..end];
    let header = {
        let rest = &line[end + 3..];
        if rest.is_empty() {
            None
        } else {
            Some(rest.trim().to_string())
        }
    };

    let parts: Vec<&str> = range_part.split(' ').collect();
    if parts.len() != 2 {
        return None;
    }

    let (old_start, old_count) = parse_range(parts[0].strip_prefix('-')?)?;
    let (new_start, new_count) = parse_range(parts[1].strip_prefix('+')?)?;

    Some((old_start, old_count, new_start, new_count, header))
}

/// Parse "start,count" or "start" (count defaults to 1).
fn parse_range(s: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = s.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((s.parse().ok()?, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DIFF: &str = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,6 @@
 fn main() {
-    println!("Hello");
+    println!("Hello, world!");
+    println!("Goodbye!");
     let x = 42;
 }
"#;

    #[test]
    fn parse_simple_diff() {
        let files = parse_unified_diff(SAMPLE_DIFF);
        assert_eq!(files.len(), 1);

        let file = &files[0];
        assert_eq!(file.old_path, "src/main.rs");
        assert_eq!(file.new_path, "src/main.rs");
        assert!(!file.is_new);
        assert!(!file.is_deleted);
        assert!(!file.is_binary);
        assert_eq!(file.hunks.len(), 1);

        let hunk = &file.hunks[0];
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 5);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_count, 6);
        // 1 context + 1 removed + 2 added + 1 context + 1 context = 6 lines
        assert_eq!(hunk.lines.len(), 6);
    }

    #[test]
    fn parse_new_file() {
        let diff = r#"diff --git a/new_file.rs b/new_file.rs
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/new_file.rs
@@ -0,0 +1,3 @@
+fn hello() {
+    println!("new!");
+}
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert!(files[0].is_new);
        assert_eq!(files[0].new_path, "new_file.rs");
        assert_eq!(files[0].hunks[0].lines.len(), 3);
    }

    #[test]
    fn parse_deleted_file() {
        let diff = r#"diff --git a/old_file.rs b/old_file.rs
deleted file mode 100644
index 1234567..0000000
--- a/old_file.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-fn old() {
-}
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert!(files[0].is_deleted);
        assert_eq!(files[0].old_path, "old_file.rs");
    }

    #[test]
    fn parse_multiple_files() {
        let diff = r#"diff --git a/a.rs b/a.rs
index 1234567..abcdefg 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 fn a() {
-    1
+    2
 }
diff --git a/b.rs b/b.rs
index 1234567..abcdefg 100644
--- a/b.rs
+++ b/b.rs
@@ -1,3 +1,3 @@
 fn b() {
-    3
+    4
 }
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].new_path, "a.rs");
        assert_eq!(files[1].new_path, "b.rs");
    }

    #[test]
    fn parse_rename() {
        let diff = r#"diff --git a/old_name.rs b/new_name.rs
similarity index 95%
rename from old_name.rs
rename to new_name.rs
index 1234567..abcdefg 100644
--- a/old_name.rs
+++ b/new_name.rs
@@ -1,3 +1,3 @@
 fn renamed() {
-    old()
+    new()
 }
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert!(files[0].is_rename);
        assert_eq!(files[0].old_path, "old_name.rs");
        assert_eq!(files[0].new_path, "new_name.rs");
    }

    #[test]
    fn parse_empty_diff() {
        let files = parse_unified_diff("");
        assert!(files.is_empty());
    }

    #[test]
    fn line_numbers_are_correct() {
        let files = parse_unified_diff(SAMPLE_DIFF);
        let hunk = &files[0].hunks[0];

        // Context line: " fn main() {"
        assert_eq!(hunk.lines[0].old_line_no, Some(1));
        assert_eq!(hunk.lines[0].new_line_no, Some(1));

        // Removed: "-    println!(\"Hello\");"
        assert_eq!(hunk.lines[1].old_line_no, Some(2));
        assert_eq!(hunk.lines[1].new_line_no, None);

        // Added: "+    println!(\"Hello, world!\");"
        assert_eq!(hunk.lines[2].old_line_no, None);
        assert_eq!(hunk.lines[2].new_line_no, Some(2));
    }

    #[test]
    fn file_diff_helpers() {
        let files = parse_unified_diff(SAMPLE_DIFF);
        let file = &files[0];
        assert_eq!(file.path(), "src/main.rs");
        assert_eq!(file.added_lines(), 2);
        assert_eq!(file.removed_lines(), 1);
    }

    #[test]
    fn parse_binary_file() {
        let diff = r#"diff --git a/image.png b/image.png
new file mode 100644
index 0000000..1234567
Binary files /dev/null and b/image.png differ
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert!(files[0].is_binary);
        assert!(files[0].is_new);
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn parse_no_newline_marker() {
        let diff = r#"diff --git a/test.rs b/test.rs
index 1234567..abcdefg 100644
--- a/test.rs
+++ b/test.rs
@@ -1,2 +1,2 @@
-old line
+new line
\ No newline at end of file
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        let hunk = &files[0].hunks[0];
        // The "\ No newline" marker should be skipped, leaving 2 real lines
        assert_eq!(hunk.lines.len(), 2);
    }

    #[test]
    fn parse_dissimilarity_index() {
        let diff = r#"diff --git a/changed.rs b/changed.rs
dissimilarity index 80%
index 1234567..abcdefg 100644
--- a/changed.rs
+++ b/changed.rs
@@ -1,1 +1,1 @@
-old
+new
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 1);
    }

    #[test]
    fn parse_empty_context_line() {
        // An empty line (no leading space) in hunk context
        let diff = "diff --git a/test.rs b/test.rs\nindex 1234567..abcdefg 100644\n--- a/test.rs\n+++ b/test.rs\n@@ -1,3 +1,4 @@\n fn a() {\n\n+    new_line();\n }\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        // Should parse without panicking
        assert!(!files[0].hunks.is_empty());
    }

    #[test]
    fn parse_hunk_header_with_function() {
        let diff = r#"diff --git a/lib.rs b/lib.rs
index 1234567..abcdefg 100644
--- a/lib.rs
+++ b/lib.rs
@@ -10,3 +10,4 @@ fn some_function() {
     let x = 1;
+    let y = 2;
     let z = 3;
 }"#;
        let files = parse_unified_diff(diff);
        let hunk = &files[0].hunks[0];
        assert_eq!(hunk.header.as_deref(), Some("fn some_function() {"));
    }

    #[test]
    fn parse_mnemonic_prefix_cw() {
        // diff.mnemonicPrefix: c/ = commit, w/ = working tree
        let diff = "diff --git c/auth.rs w/auth.rs\nindex 1234567..abcdefg 100644\n--- c/auth.rs\n+++ w/auth.rs\n@@ -1,2 +1,3 @@\n fn main() {\n+    todo!();\n }\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_path, "auth.rs");
        assert_eq!(files[0].new_path, "auth.rs");
    }

    #[test]
    fn parse_mnemonic_prefix_iw() {
        // diff.mnemonicPrefix: i/ = index, w/ = working tree
        let diff = "diff --git i/db.rs w/db.rs\nindex 1234567..abcdefg 100644\n--- i/db.rs\n+++ w/db.rs\n@@ -1,2 +1,3 @@\n fn main() {\n+    todo!();\n }\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_path, "db.rs");
        assert_eq!(files[0].new_path, "db.rs");
    }

    #[test]
    fn strip_diff_prefix_all_variants() {
        assert_eq!(strip_diff_prefix("a/file.rs"), "file.rs");
        assert_eq!(strip_diff_prefix("b/file.rs"), "file.rs");
        assert_eq!(strip_diff_prefix("c/file.rs"), "file.rs");
        assert_eq!(strip_diff_prefix("w/file.rs"), "file.rs");
        assert_eq!(strip_diff_prefix("i/file.rs"), "file.rs");
        assert_eq!(strip_diff_prefix("o/file.rs"), "file.rs");
        // Should not strip unknown prefixes
        assert_eq!(strip_diff_prefix("x/file.rs"), "x/file.rs");
        assert_eq!(strip_diff_prefix("src/file.rs"), "src/file.rs");
        // Edge cases
        assert_eq!(strip_diff_prefix("a"), "a");
        assert_eq!(strip_diff_prefix(""), "");
    }

    #[test]
    fn parse_multiple_hunks_in_one_file() {
        let diff = r#"diff --git a/multi.rs b/multi.rs
index 1234567..abcdefg 100644
--- a/multi.rs
+++ b/multi.rs
@@ -1,3 +1,3 @@
 fn first() {
-    old1()
+    new1()
 }
@@ -10,3 +10,3 @@ fn gap() {
 fn second() {
-    old2()
+    new2()
 }
"#;
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks.len(), 2);

        let h0 = &files[0].hunks[0];
        assert_eq!(h0.old_start, 1);
        assert_eq!(h0.lines.len(), 4);

        let h1 = &files[0].hunks[1];
        assert_eq!(h1.old_start, 10);
        assert_eq!(h1.header.as_deref(), Some("fn gap() {"));
        assert_eq!(h1.lines.len(), 4);
    }
}
