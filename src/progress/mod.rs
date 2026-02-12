//! Progress reporting for terminal output.
//!
//! Provides a live-updating file status display with colored checkmarks,
//! spinners, and failure indicators. Designed for interactive terminals;
//! silenced with `--no-progress`.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::Mutex;

use colored::Colorize;

/// Status of a single review task (file × agent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    /// Queued, waiting to start.
    Pending,
    /// Currently being reviewed.
    InProgress,
    /// Completed successfully.
    Done,
    /// Failed after retries.
    Failed(String),
    /// Retrying after transient error.
    Retrying { attempt: u32, max: u32, reason: String, backoff_secs: u64 },
}

/// Tracks and renders live progress for file reviews.
///
/// Thread-safe — meant to be shared across async tasks via `Arc`.
pub struct ProgressTracker {
    inner: Mutex<ProgressState>,
    /// If false, all output is suppressed.
    enabled: bool,
}

struct ProgressState {
    /// file → status (sorted for stable rendering).
    files: BTreeMap<String, TaskStatus>,
    /// Number of lines we last printed (for clearing).
    rendered_lines: usize,
    /// Agent names for the header.
    agents: Vec<String>,
}

impl ProgressTracker {
    /// Create a new progress tracker.
    ///
    /// `files` is the list of file paths being reviewed.
    /// `agents` is the list of agent profile names.
    /// `enabled` controls whether output is printed.
    pub fn new(files: &[String], agents: &[String], enabled: bool) -> Self {
        let mut file_map = BTreeMap::new();
        for f in files {
            file_map.insert(f.clone(), TaskStatus::Pending);
        }
        Self {
            inner: Mutex::new(ProgressState {
                files: file_map,
                rendered_lines: 0,
                agents: agents.to_vec(),
            }),
            enabled,
        }
    }

    /// Update the status of a file and re-render.
    pub fn update(&self, file: &str, status: TaskStatus) {
        let mut state = self.inner.lock().unwrap();
        state.files.insert(file.to_string(), status);
        if self.enabled {
            Self::render(&mut state);
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
    pub fn finish(&self, total_findings: usize) {
        if !self.enabled {
            return;
        }
        let mut state = self.inner.lock().unwrap();
        // Clear the progress display
        Self::clear_lines(state.rendered_lines);
        state.rendered_lines = 0;

        // Print final status for each file
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        for (file, status) in &state.files {
            let icon = match status {
                TaskStatus::Done => "✔".green().bold().to_string(),
                TaskStatus::Failed(_) => "✖".red().bold().to_string(),
                _ => "✔".green().bold().to_string(),
            };
            let file_display = file.dimmed();
            let status_text = match status {
                TaskStatus::Done => "done".green().to_string(),
                TaskStatus::Failed(reason) => format!("{}", reason.red()),
                _ => "done".green().to_string(),
            };
            let _ = writeln!(handle, "  {icon} {file_display} {status_text}");
        }

        // Summary line
        let _ = writeln!(handle);
        if total_findings == 0 {
            let _ = writeln!(handle, "  {} {}", "✔".green().bold(), "No issues found.".green());
        }
    }

    /// Render the current state to stderr, clearing previous output.
    fn render(state: &mut ProgressState) {
        let stderr = io::stderr();
        let mut handle = stderr.lock();

        // Clear previous lines
        Self::clear_lines(state.rendered_lines);

        let mut lines = 0;

        // Header
        let file_count = state.files.len();
        let agents_str = state.agents.join(", ");
        let _ = writeln!(
            handle,
            "  {} Reviewing {file_count} file(s) with {} [{}]",
            "▸".cyan().bold(),
            if state.agents.len() == 1 {
                format!("{} agent", state.agents.len())
            } else {
                format!("{} agents", state.agents.len())
            },
            agents_str.dimmed(),
        );
        lines += 1;

        // File list
        for (file, status) in &state.files {
            let (icon, status_text) = match status {
                TaskStatus::Pending => (
                    "○".dimmed().to_string(),
                    "waiting".dimmed().to_string(),
                ),
                TaskStatus::InProgress => (
                    "◌".cyan().bold().to_string(),
                    "reviewing…".cyan().to_string(),
                ),
                TaskStatus::Done => (
                    "✔".green().bold().to_string(),
                    "done".green().to_string(),
                ),
                TaskStatus::Failed(reason) => (
                    "✖".red().bold().to_string(),
                    reason.red().to_string(),
                ),
                TaskStatus::Retrying { attempt, max, reason, backoff_secs } => (
                    "⟳".yellow().bold().to_string(),
                    format!("{reason}, retrying in {backoff_secs}s ({attempt}/{max})").yellow().to_string(),
                ),
            };
            let _ = writeln!(handle, "    {icon} {file} {status_text}", file = file.dimmed());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_disabled_no_panic() {
        let tracker = ProgressTracker::new(
            &["file.rs".to_string()],
            &["backend".to_string()],
            false,
        );
        tracker.start();
        tracker.update("file.rs", TaskStatus::InProgress);
        tracker.update("file.rs", TaskStatus::Done);
        tracker.finish(0);
    }

    #[test]
    fn tracker_tracks_state() {
        let tracker = ProgressTracker::new(
            &["a.rs".to_string(), "b.rs".to_string()],
            &["backend".to_string()],
            false, // disabled to avoid terminal output in tests
        );
        tracker.update("a.rs", TaskStatus::InProgress);
        tracker.update("a.rs", TaskStatus::Done);
        tracker.update("b.rs", TaskStatus::Failed("API error".to_string()));

        let state = tracker.inner.lock().unwrap();
        assert_eq!(state.files["a.rs"], TaskStatus::Done);
        assert!(matches!(&state.files["b.rs"], TaskStatus::Failed(_)));
    }
}
