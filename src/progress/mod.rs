//! Progress reporting for terminal output.
//!
//! # Bounded Context: Progress Display
//!
//! Owns the live terminal status display — spinners, colored
//! checkmarks, and tool-call audit summaries. Consumed by the
//! orchestrator via `ProgressTracker`; has no knowledge of LLM
//! providers or finding content.
//!
//! Designed for interactive terminals; silenced with `--quiet`.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::Mutex;
use std::time::Instant;

use colored::Colorize;

/// Status of a single review task (file × agent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    /// Queued, waiting to start.
    Pending,
    /// Currently being reviewed.
    InProgress,
    /// Currently invoking a specific tool. Displayed as a transient
    /// substate of `InProgress`; the orchestrator switches back to
    /// `InProgress` once the tool returns.
    ToolCalling { tool: String },
    /// Completed successfully.
    Done,
    /// Failed after retries.
    Failed(String),
    /// Retrying after transient error.
    Retrying {
        attempt: u32,
        max: u32,
        reason: String,
        backoff_secs: u64,
    },
}

/// Trait for progress reporting, enabling testable and pluggable
/// progress displays (live terminal, quiet/no-op, custom CI, etc.).
pub trait ProgressReporter: Send + Sync {
    /// Update the status of a single review task (one file × agent pair).
    fn update(&self, file: &str, agent: &str, status: TaskStatus);
    /// Print the initial progress display.
    fn start(&self);
    /// Clear progress and print the final summary.
    fn finish(&self);
}

/// Tracks and renders live progress for file reviews.
///
/// Thread-safe — meant to be shared across async tasks via `Arc`.
pub struct ProgressTracker {
    inner: Mutex<ProgressState>,
    /// If false, all output is suppressed.
    enabled: bool,
}

/// Composite key identifying a single review task (file × agent).
type TaskKey = (String, String);

struct ProgressState {
    /// (file, agent) → status (sorted for stable rendering).
    tasks: BTreeMap<TaskKey, TaskStatus>,
    /// Number of lines we last printed (for clearing).
    rendered_lines: usize,
    /// Agent names for the header.
    agents: Vec<String>,
    /// Last time we rendered, for debouncing.
    last_render: Instant,
}

impl ProgressTracker {
    /// Create a new progress tracker.
    ///
    /// `files` is the list of file paths being reviewed.
    /// `agents` is the list of agent profile names.
    /// `enabled` controls whether output is printed.
    ///
    /// Pre-populates one task slot per (file × agent) combination.
    pub fn new(files: &[String], agents: &[String], enabled: bool) -> Self {
        let mut tasks = BTreeMap::new();
        for f in files {
            for a in agents {
                tasks.insert((f.clone(), a.clone()), TaskStatus::Pending);
            }
        }
        Self {
            inner: Mutex::new(ProgressState {
                tasks,
                rendered_lines: 0,
                agents: agents.to_vec(),
                last_render: Instant::now(),
            }),
            enabled,
        }
    }

    /// Update the status of a single file × agent task and re-render
    /// if enough time has elapsed.
    ///
    /// Renders immediately for terminal states (Done, Failed) to ensure
    /// the final status is always visible. For transient states, renders
    /// at most once per 100ms to avoid excessive terminal I/O.
    pub fn update(&self, file: &str, agent: &str, status: TaskStatus) {
        let is_terminal_state = matches!(status, TaskStatus::Done | TaskStatus::Failed(_));
        let mut state = self.inner.lock().unwrap();
        state
            .tasks
            .insert((file.to_string(), agent.to_string()), status);
        if self.enabled {
            let elapsed = state.last_render.elapsed();
            if is_terminal_state || elapsed.as_millis() >= 100 {
                Self::render(&mut state);
                state.last_render = Instant::now();
            }
        }
    }

    /// Print the initial header and file listing.
    pub fn start(&self) {
        if !self.enabled {
            return;
        }

        let mut state = self.inner.lock().unwrap();
        // Set all files to pending and render
        Self::render(&mut state);
    }

    /// Clear progress lines and print a final summary.
    pub fn finish(&self) {
        if !self.enabled {
            return;
        }
        let mut state = self.inner.lock().unwrap();
        // Clear the progress display
        Self::clear_lines(state.rendered_lines);
        state.rendered_lines = 0;

        // Print final status for each task (file × agent)
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let width = terminal_width();
        for ((file, agent), status) in &state.tasks {
            let icon = match status {
                TaskStatus::Done => "✔".green().bold().to_string(),
                TaskStatus::Failed(_) => "✖".red().bold().to_string(),
                _ => "✔".green().bold().to_string(),
            };
            let label = format_task_label(file, agent);
            let status_text = match status {
                TaskStatus::Done => "done".green().to_string(),
                TaskStatus::Failed(reason) => sanitize_status_text(reason).red().to_string(),
                _ => "done".green().to_string(),
            };
            let line = format!("  {icon} {label} {status_text}");
            let _ = writeln!(handle, "{}", truncate_visible(&line, width));
        }

        // Tool-call audit summary (if any tools were invoked)
        let tool_calls = crate::tools::ToolCallLog::drain();
        if !tool_calls.is_empty() {
            let _ = writeln!(handle);
            let _ = writeln!(
                handle,
                "  {} {}",
                "▸".cyan().bold(),
                format!(
                    "{} tool call{}",
                    tool_calls.len(),
                    if tool_calls.len() == 1 { "" } else { "s" }
                )
                .dimmed(),
            );
            for tc in &tool_calls {
                let duration = if tc.duration.as_millis() < 1000 {
                    format!("{}ms", tc.duration.as_millis())
                } else {
                    format!("{:.1}s", tc.duration.as_secs_f64())
                };
                let _ = writeln!(
                    handle,
                    "    {} {} {} {}",
                    "→".dimmed(),
                    tc.tool_name.cyan(),
                    tc.args_summary.dimmed(),
                    format!("({}, {})", tc.result_summary, duration).dimmed(),
                );
            }
        }

        // Summary line — just a blank line separator.
        // The "No issues found" message is handled by the output renderer.
        let _ = writeln!(handle);
    }

    /// Render the current state to stderr, clearing previous output.
    fn render(state: &mut ProgressState) {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let width = terminal_width();

        // Clear previous lines
        Self::clear_lines(state.rendered_lines);

        let mut lines = 0;

        // Header
        let file_count = unique_files(&state.tasks);
        let agents_str = state.agents.join(", ");
        let header = format!(
            "  {} Reviewing {file_count} file(s) with {} [{}]",
            "▸".cyan().bold(),
            if state.agents.len() == 1 {
                format!("{} agent", state.agents.len())
            } else {
                format!("{} agents", state.agents.len())
            },
            agents_str.dimmed(),
        );
        let _ = writeln!(handle, "{}", truncate_visible(&header, width));
        lines += 1;

        // Task list (file × agent)
        for ((file, agent), status) in &state.tasks {
            let (icon, status_text) = match status {
                TaskStatus::Pending => ("○".dimmed().to_string(), "waiting".dimmed().to_string()),
                TaskStatus::InProgress => (
                    "◌".cyan().bold().to_string(),
                    "reviewing…".cyan().to_string(),
                ),
                TaskStatus::ToolCalling { tool } => (
                    "◌".cyan().bold().to_string(),
                    format!("calling {tool}…").cyan().to_string(),
                ),
                TaskStatus::Done => ("✔".green().bold().to_string(), "done".green().to_string()),
                TaskStatus::Failed(reason) => (
                    "✖".red().bold().to_string(),
                    sanitize_status_text(reason).red().to_string(),
                ),
                TaskStatus::Retrying {
                    attempt,
                    max,
                    reason,
                    backoff_secs,
                } => (
                    "⟳".yellow().bold().to_string(),
                    format!(
                        "{}, retrying in {backoff_secs}s ({attempt}/{max})",
                        sanitize_status_text(reason)
                    )
                    .yellow()
                    .to_string(),
                ),
            };
            let label = format_task_label(file, agent);
            let line = format!("    {icon} {label} {status_text}");
            let _ = writeln!(handle, "{}", truncate_visible(&line, width));
            lines += 1;
        }

        let _ = handle.flush();
        state.rendered_lines = lines;
    }

    /// Move cursor up and clear `n` lines.
    fn clear_lines(n: usize) {
        if n == 0 {
            return;
        }
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        for _ in 0..n {
            // Move up one line and clear it
            let _ = write!(handle, "\x1b[1A\x1b[2K");
        }
        let _ = handle.flush();
    }
}

/// Collapse newlines and tabs and truncate so a status reason always renders
/// on a single terminal line. Multi-line errors (e.g. provider responses
/// containing tool-call payloads) would otherwise corrupt the cursor-based
/// redraw used by `clear_lines`.
fn sanitize_status_text(reason: &str) -> String {
    const MAX_LEN: usize = 200;
    let mut out = String::with_capacity(reason.len().min(MAX_LEN + 1));
    let mut prev_space = false;
    for ch in reason.chars() {
        let mapped = if matches!(ch, '\n' | '\r' | '\t') {
            ' '
        } else {
            ch
        };
        if mapped == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        out.push(mapped);
        if out.chars().count() >= MAX_LEN {
            out.push('…');
            break;
        }
    }
    out.trim().to_string()
}

/// Format a task label as `file (agent)` with the file dimmed.
fn format_task_label(file: &str, agent: &str) -> String {
    format!("{} {}", file.dimmed(), format!("({agent})").dimmed())
}

/// Count the number of distinct file paths across all tracked tasks.
fn unique_files(tasks: &BTreeMap<TaskKey, TaskStatus>) -> usize {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (f, _) in tasks.keys() {
        seen.insert(f.as_str());
    }
    seen.len()
}

/// Determine the current terminal width in columns. Falls back to 100 if
/// the size cannot be detected (e.g. piped output, non-tty).
fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(100)
        .max(40)
}

/// Truncate a string containing ANSI escape sequences so its visible
/// width does not exceed `max_cols`. Preserves any active SGR styling
/// by appending a reset (`\x1b[0m`) when truncation occurs.
///
/// This is critical for the cursor-based progress redraw: a single
/// long status line that wraps onto a second visual row would leave
/// stale text on the terminal because `clear_lines` only undoes
/// logical (newline-terminated) lines, not wrapped continuations.
fn truncate_visible(s: &str, max_cols: usize) -> String {
    let mut visible = 0usize;
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Copy the entire ANSI escape sequence (CSI: ESC [ ... letter)
            out.push(ch);
            if let Some(&'[') = chars.peek() {
                out.push(chars.next().unwrap());
                for c in chars.by_ref() {
                    out.push(c);
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if visible >= max_cols.saturating_sub(1) {
            // Drop remaining text; close any active styling.
            out.push('…');
            out.push_str("\x1b[0m");
            return out;
        }
        out.push(ch);
        visible += 1;
    }
    out
}

impl ProgressReporter for ProgressTracker {
    fn update(&self, file: &str, agent: &str, status: TaskStatus) {
        self.update(file, agent, status);
    }

    fn start(&self) {
        self.start();
    }

    fn finish(&self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_status_text_collapses_newlines() {
        let input = "line one\nline two\r\n\tline three";
        let out = sanitize_status_text(input);
        assert!(!out.contains('\n'));
        assert!(!out.contains('\r'));
        assert!(!out.contains('\t'));
        assert_eq!(out, "line one line two line three");
    }

    #[test]
    fn sanitize_status_text_truncates_long_input() {
        let input = "x".repeat(500);
        let out = sanitize_status_text(&input);
        // <= MAX_LEN chars plus the ellipsis marker
        assert!(out.chars().count() <= 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn tracker_disabled_no_panic() {
        let tracker =
            ProgressTracker::new(&["file.rs".to_string()], &["backend".to_string()], false);
        tracker.start();
        tracker.update("file.rs", "backend", TaskStatus::InProgress);
        tracker.update("file.rs", "backend", TaskStatus::Done);
        tracker.finish();
    }

    #[test]
    fn tracker_tracks_state() {
        let tracker = ProgressTracker::new(
            &["a.rs".to_string(), "b.rs".to_string()],
            &["backend".to_string()],
            false,
        );
        tracker.update("a.rs", "backend", TaskStatus::InProgress);
        tracker.update("a.rs", "backend", TaskStatus::Done);
        tracker.update(
            "b.rs",
            "backend",
            TaskStatus::Failed("API error".to_string()),
        );

        let state = tracker.inner.lock().unwrap();
        assert_eq!(
            state.tasks[&("a.rs".to_string(), "backend".to_string())],
            TaskStatus::Done
        );
        assert!(matches!(
            &state.tasks[&("b.rs".to_string(), "backend".to_string())],
            TaskStatus::Failed(_)
        ));
    }

    #[test]
    fn tracker_retrying_status() {
        let tracker =
            ProgressTracker::new(&["retry.rs".to_string()], &["backend".to_string()], false);
        tracker.update(
            "retry.rs",
            "backend",
            TaskStatus::Retrying {
                attempt: 1,
                max: 3,
                reason: "rate limited".to_string(),
                backoff_secs: 10,
            },
        );

        let state = tracker.inner.lock().unwrap();
        match &state.tasks[&("retry.rs".to_string(), "backend".to_string())] {
            TaskStatus::Retrying {
                attempt,
                max,
                reason,
                backoff_secs,
            } => {
                assert_eq!(*attempt, 1);
                assert_eq!(*max, 3);
                assert_eq!(reason, "rate limited");
                assert_eq!(*backoff_secs, 10);
            }
            other => panic!("expected Retrying, got {other:?}"),
        }
    }

    #[test]
    fn tracker_empty_files_no_panic() {
        let tracker = ProgressTracker::new(&[], &[], false);
        tracker.start();
        tracker.finish();
    }

    #[test]
    fn tracker_finish_with_findings_no_panic() {
        let tracker = ProgressTracker::new(&["a.rs".to_string()], &["backend".to_string()], false);
        tracker.update("a.rs", "backend", TaskStatus::Done);
        tracker.finish();
    }

    #[test]
    fn tracker_multiple_agents_pre_populates_cartesian_product() {
        let tracker = ProgressTracker::new(
            &["a.rs".to_string()],
            &["backend".to_string(), "security".to_string()],
            false,
        );
        let state = tracker.inner.lock().unwrap();
        assert_eq!(state.agents.len(), 2);
        // 1 file × 2 agents = 2 tasks pre-populated.
        assert_eq!(state.tasks.len(), 2);
        assert!(
            state
                .tasks
                .contains_key(&("a.rs".to_string(), "backend".to_string()))
        );
        assert!(
            state
                .tasks
                .contains_key(&("a.rs".to_string(), "security".to_string()))
        );
    }

    #[test]
    fn tracker_update_unknown_task_adds_it() {
        let tracker = ProgressTracker::new(&["a.rs".to_string()], &["backend".to_string()], false);
        // Updating an (file, agent) pair not in the initial list should insert it.
        tracker.update("unknown.rs", "frontend", TaskStatus::Done);
        let state = tracker.inner.lock().unwrap();
        assert_eq!(
            state.tasks[&("unknown.rs".to_string(), "frontend".to_string())],
            TaskStatus::Done
        );
    }

    #[test]
    fn truncate_visible_keeps_short_lines_intact() {
        let s = "hello world";
        assert_eq!(truncate_visible(s, 80), "hello world");
    }

    #[test]
    fn truncate_visible_caps_long_lines_with_ellipsis() {
        let s = "x".repeat(120);
        let out = truncate_visible(&s, 40);
        // Ends with ellipsis + ANSI reset, no embedded newlines.
        assert!(out.contains('…'));
        assert!(!out.contains('\n'));
    }

    #[test]
    fn truncate_visible_preserves_ansi_escapes_in_visible_portion() {
        let s = "\x1b[31mred\x1b[0m and \x1b[32mgreen\x1b[0m text";
        // Plenty of room — should keep all escapes.
        let out = truncate_visible(s, 80);
        assert!(out.contains("\x1b[31m"));
        assert!(out.contains("\x1b[32m"));
    }
}
