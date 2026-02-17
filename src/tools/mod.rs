//! Agentic tools for LLM-driven codebase exploration.
//!
//! These tools allow the LLM to explore the repository when
//! running in agentic mode (`--agent`). Each tool implements
//! rig-core's `Tool` trait for native tool calling support.
//!
//! In addition to the built-in tools, users can define custom
//! command-line tools in their agent profile's YAML frontmatter.
//! See [`custom_command::CustomCommandTool`] for details.
//!
//! ## Tool-call audit log
//!
//! Every tool invocation is recorded in a process-global
//! [`ToolCallLog`] so that the progress display and post-review
//! summary can show what the LLM explored. Tools call
//! [`ToolCallLog::record`] at the start of their `call()` method.

pub mod custom_command;
pub mod list_directory;
pub mod read_file;
pub mod search_text;

// Re-export the rig Tool wrapper types
pub use custom_command::CustomCommandTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use search_text::SearchTextTool;

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A single recorded tool invocation.
#[derive(Debug, Clone)]
pub struct ToolCallEntry {
    /// Name of the tool (e.g. `read_file`, `search_text`, `run_tests`).
    pub tool_name: String,
    /// Short summary of the arguments (e.g. `src/main.rs`, `"fn main"`).
    pub args_summary: String,
    /// Short summary of the result (e.g. `1.2KB`, `3 results`, `exit 0`).
    pub result_summary: String,
    /// Wall-clock duration of the call.
    pub duration: Duration,
}

/// Process-global append-only log of tool invocations.
///
/// Tools write here during their `call()` method. The progress display
/// and post-review summary read from here.
pub struct ToolCallLog {
    entries: Mutex<Vec<ToolCallEntry>>,
}

/// Global tool call log instance.
static TOOL_CALL_LOG: std::sync::LazyLock<ToolCallLog> =
    std::sync::LazyLock::new(ToolCallLog::new);

impl ToolCallLog {
    /// Create a new empty log.
    const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Record a tool invocation.
    pub fn record(entry: ToolCallEntry) {
        TOOL_CALL_LOG
            .entries
            .lock()
            .expect("tool call log poisoned")
            .push(entry);
    }

    /// Take all recorded entries, draining the log.
    pub fn drain() -> Vec<ToolCallEntry> {
        TOOL_CALL_LOG
            .entries
            .lock()
            .expect("tool call log poisoned")
            .drain(..)
            .collect()
    }

    /// Read all recorded entries without clearing.
    pub fn snapshot() -> Vec<ToolCallEntry> {
        TOOL_CALL_LOG
            .entries
            .lock()
            .expect("tool call log poisoned")
            .clone()
    }
}

/// Convenience helper: time a tool call and record the result.
///
/// Returns `(start_instant, ())` — call `finish_tool_call` with the
/// start instant after the call completes.
pub fn start_tool_call() -> Instant {
    Instant::now()
}

/// Complete a tool call recording.
pub fn finish_tool_call(
    start: Instant,
    tool_name: &str,
    args_summary: impl Into<String>,
    result_summary: impl Into<String>,
) {
    ToolCallLog::record(ToolCallEntry {
        tool_name: tool_name.to_string(),
        args_summary: args_summary.into(),
        result_summary: result_summary.into(),
        duration: start.elapsed(),
    });
}
