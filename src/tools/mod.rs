//! Agentic tools for LLM-driven codebase exploration.
//!
//! # Bounded Context: Tool Execution
//!
//! Owns the built-in tool implementations (`ReadFileTool`,
//! `SearchTextTool`, `ListDirectoryTool`) and the `CustomCommandTool`
//! runtime. Each tool implements rig-core's `Tool` trait. The
//! `ToolCallLog` audit log also lives here.
//!
//! Tools execute filesystem and subprocess operations on behalf
//! of the LLM — they never interpret review findings or diffs.
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

use crossbeam_queue::SegQueue;
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
/// Uses a lock-free `SegQueue` so `record()` never blocks under
/// parallel agentic tool use.
pub struct ToolCallLog;

/// Global lock-free tool call log.
static TOOL_CALL_QUEUE: SegQueue<ToolCallEntry> = SegQueue::new();

impl ToolCallLog {
    /// Record a tool invocation (lock-free push).
    pub fn record(entry: ToolCallEntry) {
        TOOL_CALL_QUEUE.push(entry);
    }

    /// Take all recorded entries, draining the log.
    pub fn drain() -> Vec<ToolCallEntry> {
        let mut entries = Vec::new();
        while let Some(entry) = TOOL_CALL_QUEUE.pop() {
            entries.push(entry);
        }
        entries
    }

    /// Read all recorded entries without clearing.
    ///
    /// Note: this drains and re-pushes entries, so it is not truly
    /// non-destructive under concurrent writes. Use only when no
    /// tools are actively running (e.g., post-review summary).
    pub fn snapshot() -> Vec<ToolCallEntry> {
        let entries = Self::drain();
        for entry in &entries {
            TOOL_CALL_QUEUE.push(entry.clone());
        }
        entries
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
