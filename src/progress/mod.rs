//! Progress reporting for terminal output.
//!
//! # Bounded Context: Progress Display
//!
//! Owns the live terminal status display — one row per file with
//! inline colored agent badges, plus a `Tool calls` pane below
//! showing recent and in-flight tool activity. A background ticker
//! re-renders periodically so elapsed counters and the in-flight
//! spinner glyph keep animating between events.
//!
//! Designed for interactive terminals; silenced with `--quiet`.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use colored::Colorize;

/// Default cap on the number of entries kept in the live tool-call
/// log. Plan note: should be `max(8, max_concurrent)`. Callers that
/// want a different cap can use [`ProgressTracker::with_options`].
const TOOL_LOG_DEFAULT_CAP: usize = 8;

/// Background re-render cadence. Low enough that elapsed counters
/// and the in-flight spinner animate smoothly; high enough that we
/// don't repaint the terminal in tight bursts.
const RENDER_TICK: Duration = Duration::from_millis(150);

/// Braille-pattern spinner frames used for in-flight tool entries.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

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

impl TaskStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Failed(_))
    }
    fn is_active(&self) -> bool {
        matches!(
            self,
            TaskStatus::InProgress | TaskStatus::ToolCalling { .. } | TaskStatus::Retrying { .. }
        )
    }
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
    /// Record that an agent started invoking a tool. Default: no-op.
    fn tool_started(&self, _file: &str, _agent: &str, _tool: &str, _args_summary: &str) {}
    /// Record that an agent's tool call completed. Default: no-op.
    fn tool_finished(
        &self,
        _file: &str,
        _agent: &str,
        _tool: &str,
        _ok: bool,
        _duration: Duration,
    ) {
    }
}

/// Tracks and renders live progress for file reviews.
///
/// Thread-safe — meant to be shared across async tasks via `Arc`.
pub struct ProgressTracker {
    inner: Arc<Mutex<ProgressState>>,
    /// If false, all output is suppressed.
    enabled: bool,
    /// If false, the `Tool calls` pane is hidden entirely.
    show_tool_log: bool,
    /// Background re-render thread, joined on `finish()` / `Drop`.
    ticker: Mutex<Option<JoinHandle<()>>>,
    /// Signal flag the ticker checks each iteration.
    stop: Arc<AtomicBool>,
}

/// Composite key identifying a single review task (file × agent).
type TaskKey = (String, String);

#[derive(Debug, Clone)]
struct TaskRecord {
    status: TaskStatus,
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
}

impl TaskRecord {
    fn pending() -> Self {
        Self {
            status: TaskStatus::Pending,
            started_at: None,
            finished_at: None,
        }
    }
}

struct ProgressState {
    /// Files in the order they were registered (preserves insertion
    /// order for stable per-file rows).
    files: Vec<String>,
    /// Agent names (column order for badges in each file row).
    agents: Vec<String>,
    /// (file, agent) → record. Sorted via BTreeMap so the final
    /// summary lists tasks in a stable order.
    tasks: BTreeMap<TaskKey, TaskRecord>,
    /// Number of lines we last printed (for clearing).
    rendered_lines: usize,
    /// Last time we rendered, for debouncing.
    last_render: Instant,
    /// Wall-clock start of the run, anchoring the header elapsed
    /// counter and the spinner phase.
    started_at: Instant,
    /// Ring buffer of recent tool-call activity. The most recent
    /// entries (running and completed) bubble to the front. Capped
    /// at `log_cap`; persists across waves.
    live_log: VecDeque<ToolActivity>,
    /// Cap for `live_log`.
    log_cap: usize,
}

/// One tool-call entry shown in the live activity pane.
#[derive(Debug, Clone)]
struct ToolActivity {
    file: String,
    agent: String,
    tool: String,
    args_summary: String,
    /// `None` while in flight; `Some((ok, duration))` once finished.
    finished: Option<(bool, Duration)>,
}

impl ProgressTracker {
    /// Create a new progress tracker with default options.
    ///
    /// Pre-populates one task slot per (file × agent) combination.
    pub fn new(files: &[String], agents: &[String], enabled: bool) -> Self {
        Self::with_options(files, agents, enabled, true, TOOL_LOG_DEFAULT_CAP)
    }

    /// Create a new progress tracker with explicit options.
    ///
    /// `show_tool_log` hides the `Tool calls` pane entirely when false
    /// (e.g. when `--agent` is off and no tools can fire). `log_cap`
    /// is the ring-buffer size for the tool log.
    pub fn with_options(
        files: &[String],
        agents: &[String],
        enabled: bool,
        show_tool_log: bool,
        log_cap: usize,
    ) -> Self {
        let mut tasks = BTreeMap::new();
        for f in files {
            for a in agents {
                tasks.insert((f.clone(), a.clone()), TaskRecord::pending());
            }
        }
        let now = Instant::now();
        let inner = Arc::new(Mutex::new(ProgressState {
            files: files.to_vec(),
            agents: agents.to_vec(),
            tasks,
            rendered_lines: 0,
            last_render: now,
            started_at: now,
            live_log: VecDeque::with_capacity(log_cap.max(1) + 1),
            log_cap: log_cap.max(1),
        }));
        Self {
            inner,
            enabled,
            show_tool_log,
            ticker: Mutex::new(None),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Update the status of a single file × agent task and re-render
    /// if enough time has elapsed.
    pub fn update(&self, file: &str, agent: &str, status: TaskStatus) {
        let mut state = self.inner.lock().unwrap();
        let key = (file.to_string(), agent.to_string());
        // Insert new file/agent if first time seeing them so the
        // header/badge layout reflects them in subsequent renders.
        if !state.files.iter().any(|f| f == file) {
            state.files.push(file.to_string());
        }
        if !state.agents.iter().any(|a| a == agent) {
            state.agents.push(agent.to_string());
        }
        let entry = state.tasks.entry(key).or_insert_with(TaskRecord::pending);
        let was_terminal = entry.status.is_terminal();
        let now_terminal = status.is_terminal();
        if status.is_active() && entry.started_at.is_none() {
            entry.started_at = Some(Instant::now());
        }
        if now_terminal && !was_terminal {
            entry.finished_at = Some(Instant::now());
        }
        entry.status = status;

        if self.enabled {
            let elapsed = state.last_render.elapsed();
            // Render immediately on terminal transitions; otherwise
            // debounced. The background ticker covers the gaps.
            if now_terminal || elapsed.as_millis() >= 80 {
                Self::render(&mut state, self.show_tool_log);
                state.last_render = Instant::now();
            }
        }
    }

    /// Push a "tool started" entry into the live activity pane.
    pub fn tool_started(&self, file: &str, agent: &str, tool: &str, args_summary: &str) {
        let mut state = self.inner.lock().unwrap();
        state.live_log.push_front(ToolActivity {
            file: file.to_string(),
            agent: agent.to_string(),
            tool: tool.to_string(),
            args_summary: args_summary.to_string(),
            finished: None,
        });
        let cap = state.log_cap;
        // Cap the buffer; evict oldest *finished* entry first to keep
        // running entries visible.
        while state.live_log.len() > cap {
            let evict_idx = state
                .live_log
                .iter()
                .rposition(|t| t.finished.is_some())
                .unwrap_or(state.live_log.len() - 1);
            state.live_log.remove(evict_idx);
        }
        if self.enabled {
            Self::render(&mut state, self.show_tool_log);
            state.last_render = Instant::now();
        }
    }

    /// Mark the most recent matching in-flight tool entry as finished.
    pub fn tool_finished(&self, file: &str, agent: &str, tool: &str, ok: bool, duration: Duration) {
        let mut state = self.inner.lock().unwrap();
        if let Some(entry) = state
            .live_log
            .iter_mut()
            .find(|t| t.finished.is_none() && t.agent == agent && t.tool == tool && t.file == file)
        {
            entry.finished = Some((ok, duration));
        }
        if self.enabled {
            Self::render(&mut state, self.show_tool_log);
            state.last_render = Instant::now();
        }
    }

    /// Print the initial header and file listing, and start the
    /// background re-render ticker.
    pub fn start(&self) {
        if !self.enabled {
            return;
        }
        {
            let mut state = self.inner.lock().unwrap();
            state.started_at = Instant::now();
            Self::render(&mut state, self.show_tool_log);
            state.last_render = Instant::now();
        }
        // Background ticker so elapsed/spinner animate without events.
        let inner = Arc::clone(&self.inner);
        let stop = Arc::clone(&self.stop);
        let show_tool_log = self.show_tool_log;
        let handle = thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(RENDER_TICK);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut state) = inner.lock() else {
                    continue;
                };
                if state.tasks.is_empty() {
                    continue;
                }
                let any_running = state.tasks.values().any(|r| r.status.is_active());
                let any_inflight = state.live_log.iter().any(|e| e.finished.is_none());
                if !any_running && !any_inflight {
                    // Nothing animating; skip the redraw but keep the
                    // ticker alive in case work resumes.
                    continue;
                }
                Self::render(&mut state, show_tool_log);
                state.last_render = Instant::now();
            }
        });
        *self.ticker.lock().unwrap() = Some(handle);
    }

    /// Clear progress lines, stop the ticker, and print a final summary.
    pub fn finish(&self) {
        // Stop the ticker first so it can't repaint during the summary.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.ticker.lock().unwrap().take() {
            let _ = h.join();
        }
        if !self.enabled {
            return;
        }
        let mut state = self.inner.lock().unwrap();
        Self::clear_lines(state.rendered_lines);
        state.rendered_lines = 0;

        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let width = terminal_width();

        // ── Header counts line ─────────────────────────────────────
        let total = state.tasks.len();
        let done = state
            .tasks
            .values()
            .filter(|r| matches!(r.status, TaskStatus::Done))
            .count();
        let failed = state
            .tasks
            .values()
            .filter(|r| matches!(r.status, TaskStatus::Failed(_)))
            .count();
        let elapsed = format_duration_secs(state.started_at.elapsed());
        let summary_tail = if failed > 0 {
            format!("· {done} done · {failed} failed · {elapsed}")
        } else {
            format!("· {done}/{total} done · {elapsed}")
        };
        let header = format!(
            "  {} {} {}",
            "▸".cyan().bold(),
            format!("{} task{}", total, if total == 1 { "" } else { "s" }).bold(),
            summary_tail.dimmed(),
        );
        let _ = writeln!(handle, "{}", truncate_visible(&header, width));

        // ── Per-file rows with badges (same layout as live mode) ───
        let file_col = state
            .files
            .iter()
            .map(|f| f.chars().count())
            .max()
            .unwrap_or(0)
            .min(60);
        for file in &state.files {
            let line = format_file_row(file, &state.agents, &state.tasks, file_col);
            let _ = writeln!(handle, "{}", truncate_visible(&line, width));
        }

        // ── Failures: surface error messages once each ─────────────
        let mut printed_fail_header = false;
        for (key, rec) in &state.tasks {
            if let TaskStatus::Failed(reason) = &rec.status {
                if !printed_fail_header {
                    let _ = writeln!(handle);
                    let _ = writeln!(handle, "    {}", "Failed tasks".red().bold());
                    printed_fail_header = true;
                }
                let line = format!(
                    "    {} {} {} {}",
                    "✖".red().bold(),
                    key.0.as_str().bold(),
                    format!("[{}]", key.1).dimmed(),
                    sanitize_status_text(reason).red(),
                );
                let _ = writeln!(handle, "{}", truncate_visible(&line, width));
            }
        }

        // ── Tool-call rollup ───────────────────────────────────────
        // Total only — the per-call detail is already on screen during
        // the run via the live `Tool calls` pane and the audit log,
        // so listing every entry here would just duplicate output and
        // grow unbounded for long agentic runs.
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
        }

        let _ = writeln!(handle);
    }

    /// Render the current state to stderr, clearing previous output.
    ///
    /// Layout:
    /// ```text
    ///   6 tasks · 5/6 running · 12.4s
    ///     login.jsx   [▣ backend  ▣ frontend  ✔ security]   2.1s
    ///     server.py   [✔ backend  ◆ frontend  ▣ security]   3.4s
    ///
    ///     Tool calls
    ///     ▸ backend·login.jsx     read_file login.jsx       3ms
    ///     ▸ frontend·server.py    read_file server.py       ⠋ running
    /// ```
    fn render(state: &mut ProgressState, show_tool_log: bool) {
        let stderr = io::stderr();
        let mut handle = stderr.lock();
        let width = terminal_width();

        Self::clear_lines(state.rendered_lines);
        let mut lines = 0;

        // ── Header counts line ─────────────────────────────────────
        let total = state.tasks.len();
        let running = state
            .tasks
            .values()
            .filter(|r| r.status.is_active())
            .count();
        let elapsed = format_duration_secs(state.started_at.elapsed());
        let header = format!(
            "  {} {} {}",
            "▸".cyan().bold(),
            format!("{} task{}", total, if total == 1 { "" } else { "s" }).bold(),
            format!("· {running}/{total} running · {elapsed}").dimmed(),
        );
        let _ = writeln!(handle, "{}", truncate_visible(&header, width));
        lines += 1;

        // ── Per-file rows with inline agent badges ─────────────────
        let file_col = state
            .files
            .iter()
            .map(|f| f.chars().count())
            .max()
            .unwrap_or(0)
            .min(60);
        for file in &state.files {
            let line = format_file_row(file, &state.agents, &state.tasks, file_col);
            let _ = writeln!(handle, "{}", truncate_visible(&line, width));
            lines += 1;
        }

        // ── Tool log pane ──────────────────────────────────────────
        if show_tool_log && !state.live_log.is_empty() {
            let _ = writeln!(handle);
            lines += 1;
            let _ = writeln!(handle, "  {} {}", "▸".cyan().bold(), "Tool calls".bold());
            lines += 1;
            // Pad agent and file into their own columns so neither
            // looks like part of the other.
            let agent_col = state
                .live_log
                .iter()
                .map(|t| t.agent.chars().count())
                .max()
                .unwrap_or(0)
                .min(20);
            let file_col = state
                .live_log
                .iter()
                .map(|t| t.file.chars().count())
                .max()
                .unwrap_or(0)
                .min(40);
            let spinner = spinner_frame(state.started_at);
            for entry in state.live_log.iter() {
                let line = format_tool_activity(entry, agent_col, file_col, spinner);
                let _ = writeln!(handle, "{}", truncate_visible(&line, width));
                lines += 1;
            }
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
            let _ = write!(handle, "\x1b[1A\x1b[2K");
        }
        let _ = handle.flush();
    }
}

impl Drop for ProgressTracker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.ticker.lock().unwrap().take() {
            let _ = h.join();
        }
    }
}

/// Collapse newlines and tabs and truncate so a status reason always renders
/// on a single terminal line.
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

/// Right-pad a plain string (no ANSI escapes) to `width` columns.
fn pad_visible(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + (width - len));
        out.push_str(s);
        for _ in len..width {
            out.push(' ');
        }
        out
    }
}

/// Render one file row with inline per-agent status badges plus an
/// elapsed-time tail.
///
/// Layout: `<file>   [<badge> <agent>  <badge> <agent>  …]   <elapsed>`.
/// `agent_order` is the column order; missing (file, agent) entries
/// render as `▢ pending`.
fn format_file_row(
    file: &str,
    agent_order: &[String],
    tasks: &BTreeMap<TaskKey, TaskRecord>,
    file_col: usize,
) -> String {
    let mut earliest: Option<Instant> = None;
    let mut latest_done: Option<Instant> = None;
    let mut all_terminal = true;
    let mut any_started = false;
    let mut badges = String::new();
    for (i, agent) in agent_order.iter().enumerate() {
        let rec = tasks.get(&(file.to_string(), agent.clone()));
        let badge = match rec.map(|r| &r.status) {
            None | Some(TaskStatus::Pending) => "▢".dimmed().to_string(),
            Some(TaskStatus::InProgress) => "▣".cyan().bold().to_string(),
            Some(TaskStatus::ToolCalling { .. }) => "◆".cyan().bold().to_string(),
            Some(TaskStatus::Retrying { .. }) => "⟳".yellow().bold().to_string(),
            Some(TaskStatus::Done) => "✔".green().bold().to_string(),
            Some(TaskStatus::Failed(_)) => "✖".red().bold().to_string(),
        };
        let sep = if i == 0 { "" } else { "  " };
        badges.push_str(&format!("{sep}{badge} {}", agent.dimmed()));
        if let Some(r) = rec {
            if let Some(t) = r.started_at {
                any_started = true;
                earliest = Some(earliest.map_or(t, |e| e.min(t)));
            }
            if let Some(t) = r.finished_at {
                latest_done = Some(latest_done.map_or(t, |e| e.max(t)));
            }
            if !r.status.is_terminal() {
                all_terminal = false;
            }
        } else {
            all_terminal = false;
        }
    }

    let elapsed_str = if !any_started {
        String::new()
    } else if all_terminal {
        let dur = match (earliest, latest_done) {
            (Some(s), Some(e)) if e >= s => e - s,
            _ => Duration::ZERO,
        };
        format_duration(dur)
    } else if let Some(s) = earliest {
        format_duration(s.elapsed())
    } else {
        String::new()
    };

    let file_padded = pad_visible(file, file_col);
    if elapsed_str.is_empty() {
        format!("    {} [{badges}]", file_padded.bold())
    } else {
        format!(
            "    {} [{badges}]  {}",
            file_padded.bold(),
            elapsed_str.dimmed()
        )
    }
}

/// Format a single tool-activity entry for the live activity pane.
///
/// Layout: `→ <agent>   <file>   <tool> <args>  <trailer>`. Agent and
/// file are padded into their own columns so they don't visually
/// merge into a single token. Section headers use cyan `▸`.
fn format_tool_activity(
    entry: &ToolActivity,
    agent_col: usize,
    file_col: usize,
    spinner: char,
) -> String {
    let agent_padded = pad_visible(&entry.agent, agent_col);
    let file_padded = pad_visible(&entry.file, file_col);
    let tool_args = if entry.args_summary.is_empty() {
        entry.tool.cyan().to_string()
    } else {
        format!("{} {}", entry.tool.cyan(), entry.args_summary.dimmed())
    };
    let trailer = match entry.finished {
        None => format!("{spinner} running").cyan().to_string(),
        Some((true, d)) => format_duration(d).green().to_string(),
        Some((false, d)) => format!("error · {}", format_duration(d)).red().to_string(),
    };
    format!(
        "    {} {}   {}   {tool_args}  {trailer}",
        "→".dimmed(),
        agent_padded.dimmed(),
        file_padded.dimmed(),
    )
}

/// Compact human-readable duration with sub-second granularity
/// (e.g. `12ms`, `1.2s`).
fn format_duration(d: Duration) -> String {
    if d.as_millis() < 1000 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

/// Always-seconds duration rendering for the header counter
/// (e.g. `12.4s`, `2m 03s`).
fn format_duration_secs(d: Duration) -> String {
    let total = d.as_secs_f64();
    if total < 60.0 {
        format!("{total:.1}s")
    } else {
        let mins = (total / 60.0) as u64;
        let secs = (total - (mins as f64) * 60.0) as u64;
        format!("{mins}m {secs:02}s")
    }
}

/// Pick a spinner glyph based on time since the run started.
fn spinner_frame(started_at: Instant) -> char {
    let idx = (started_at.elapsed().as_millis() / 80) as usize % SPINNER_FRAMES.len();
    SPINNER_FRAMES[idx]
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
fn truncate_visible(s: &str, max_cols: usize) -> String {
    let mut visible = 0usize;
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
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

    fn tool_started(&self, file: &str, agent: &str, tool: &str, args_summary: &str) {
        self.tool_started(file, agent, tool, args_summary);
    }

    fn tool_finished(&self, file: &str, agent: &str, tool: &str, ok: bool, duration: Duration) {
        self.tool_finished(file, agent, tool, ok, duration);
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
            state.tasks[&("a.rs".to_string(), "backend".to_string())].status,
            TaskStatus::Done
        );
        assert!(matches!(
            state.tasks[&("b.rs".to_string(), "backend".to_string())].status,
            TaskStatus::Failed(_)
        ));
    }

    #[test]
    fn tracker_records_started_and_finished_at() {
        let tracker = ProgressTracker::new(&["a.rs".to_string()], &["backend".to_string()], false);
        tracker.update("a.rs", "backend", TaskStatus::InProgress);
        tracker.update("a.rs", "backend", TaskStatus::Done);
        let state = tracker.inner.lock().unwrap();
        let rec = &state.tasks[&("a.rs".to_string(), "backend".to_string())];
        assert!(
            rec.started_at.is_some(),
            "should record started_at on InProgress"
        );
        assert!(
            rec.finished_at.is_some(),
            "should record finished_at on Done"
        );
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
        match &state.tasks[&("retry.rs".to_string(), "backend".to_string())].status {
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
        tracker.update("unknown.rs", "frontend", TaskStatus::Done);
        let state = tracker.inner.lock().unwrap();
        assert_eq!(
            state.tasks[&("unknown.rs".to_string(), "frontend".to_string())].status,
            TaskStatus::Done
        );
        assert!(state.files.iter().any(|f| f == "unknown.rs"));
        assert!(state.agents.iter().any(|a| a == "frontend"));
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
        assert!(out.contains('…'));
        assert!(!out.contains('\n'));
    }

    #[test]
    fn truncate_visible_preserves_ansi_escapes_in_visible_portion() {
        let s = "\x1b[31mred\x1b[0m and \x1b[32mgreen\x1b[0m text";
        let out = truncate_visible(s, 80);
        assert!(out.contains("\x1b[31m"));
        assert!(out.contains("\x1b[32m"));
    }

    #[test]
    fn tool_started_records_in_recent_activity() {
        let tracker = ProgressTracker::new(&["a.rs".to_string()], &["backend".to_string()], false);
        tracker.tool_started("a.rs", "backend", "read_file", "src/main.rs");
        let state = tracker.inner.lock().unwrap();
        assert_eq!(state.live_log.len(), 1);
        let entry = &state.live_log[0];
        assert_eq!(entry.file, "a.rs");
        assert_eq!(entry.agent, "backend");
        assert_eq!(entry.tool, "read_file");
        assert_eq!(entry.args_summary, "src/main.rs");
        assert!(entry.finished.is_none(), "should still be in flight");
    }

    #[test]
    fn tool_finished_marks_matching_entry() {
        let tracker = ProgressTracker::new(&["a.rs".to_string()], &["backend".to_string()], false);
        tracker.tool_started("a.rs", "backend", "read_file", "src/main.rs");
        tracker.tool_finished(
            "a.rs",
            "backend",
            "read_file",
            true,
            std::time::Duration::from_millis(42),
        );
        let state = tracker.inner.lock().unwrap();
        let entry = &state.live_log[0];
        let (ok, dur) = entry.finished.expect("should be finished");
        assert!(ok);
        assert_eq!(dur, std::time::Duration::from_millis(42));
    }

    #[test]
    fn tool_finished_only_matches_same_file() {
        // Two agents call read_file for different files in parallel;
        // a finish for one must not collapse onto the other entry.
        let tracker = ProgressTracker::new(
            &["a.rs".to_string(), "b.rs".to_string()],
            &["backend".to_string()],
            false,
        );
        tracker.tool_started("a.rs", "backend", "read_file", "");
        tracker.tool_started("b.rs", "backend", "read_file", "");
        tracker.tool_finished(
            "b.rs",
            "backend",
            "read_file",
            true,
            std::time::Duration::from_millis(5),
        );
        let state = tracker.inner.lock().unwrap();
        let a = state
            .live_log
            .iter()
            .find(|e| e.file == "a.rs")
            .expect("a.rs entry");
        let b = state
            .live_log
            .iter()
            .find(|e| e.file == "b.rs")
            .expect("b.rs entry");
        assert!(a.finished.is_none(), "a.rs entry should still be running");
        assert!(b.finished.is_some(), "b.rs entry should be finished");
    }

    #[test]
    fn live_log_caps_at_capacity_and_evicts_oldest_finished() {
        let tracker = ProgressTracker::with_options(
            &["a.rs".to_string()],
            &["backend".to_string()],
            false,
            true,
            5,
        );
        for i in 0..(5 + 3) {
            let tool = format!("tool_{i}");
            tracker.tool_started("a.rs", "backend", &tool, "");
            tracker.tool_finished(
                "a.rs",
                "backend",
                &tool,
                true,
                std::time::Duration::from_millis(1),
            );
        }
        let state = tracker.inner.lock().unwrap();
        assert_eq!(state.live_log.len(), 5);
        assert_eq!(state.live_log.front().unwrap().tool, "tool_7");
    }

    #[test]
    fn live_log_eviction_prefers_finished_over_running() {
        let tracker = ProgressTracker::with_options(
            &["a.rs".to_string()],
            &["backend".to_string()],
            false,
            true,
            5,
        );
        tracker.tool_started("a.rs", "backend", "long_running", "args");
        for i in 0..(5 + 1) {
            let tool = format!("done_{i}");
            tracker.tool_started("a.rs", "backend", &tool, "");
            tracker.tool_finished(
                "a.rs",
                "backend",
                &tool,
                true,
                std::time::Duration::from_millis(1),
            );
        }
        let state = tracker.inner.lock().unwrap();
        assert_eq!(state.live_log.len(), 5);
        assert!(
            state
                .live_log
                .iter()
                .any(|t| t.tool == "long_running" && t.finished.is_none()),
            "in-flight entry should not be evicted while finished entries exist"
        );
    }

    #[test]
    fn format_tool_activity_shows_running_and_finished_states() {
        let running = ToolActivity {
            file: "a.rs".to_string(),
            agent: "backend".to_string(),
            tool: "read_file".to_string(),
            args_summary: "src/main.rs".to_string(),
            finished: None,
        };
        let line = format_tool_activity(&running, 8, 6, '⠋');
        assert!(line.contains("running"));
        assert!(line.contains("backend"));
        assert!(line.contains("a.rs"));
        // Columns must not be glued together with `·` or `-`.
        assert!(!line.contains("backend·"));
        assert!(!line.contains("backend-a.rs"));

        let finished = ToolActivity {
            file: "a.rs".to_string(),
            agent: "backend".to_string(),
            tool: "read_file".to_string(),
            args_summary: "src/main.rs".to_string(),
            finished: Some((true, std::time::Duration::from_millis(123))),
        };
        let line = format_tool_activity(&finished, 8, 6, '⠋');
        assert!(line.contains("123ms"));
    }

    #[test]
    fn format_duration_secs_under_minute() {
        assert_eq!(format_duration_secs(Duration::from_millis(2400)), "2.4s");
    }

    #[test]
    fn format_duration_secs_over_minute() {
        let d = Duration::from_secs(125);
        assert_eq!(format_duration_secs(d), "2m 05s");
    }
}
