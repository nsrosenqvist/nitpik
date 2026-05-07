//! Agentic tools for LLM-driven codebase exploration.
//!
//! # Bounded Context: Tool Execution
//!
//! Owns the built-in tool implementations (`ReadFileTool`,
//! `SearchTextTool`, `ListDirectoryTool`) and the `CustomCommandTool`
//! runtime. Each tool implements rig-core's `Tool` trait.
//!
//! Tools execute filesystem and subprocess operations on behalf of the
//! LLM — they never interpret review findings or diffs.
//!
//! ## Tool-call accounting
//!
//! Every tool invocation calls [`finish_tool_call`], which forwards
//! the entry to the audit module's per-task buffer. The progress
//! module tracks counts independently via the agent loop's event
//! stream, so this layer has no global state of its own.

pub mod budget;
pub mod custom_command;
pub mod format;
pub mod glob;
pub mod list_directory;
pub mod memo;
pub mod read_file;
pub mod read_files;
pub mod search_text;
pub mod submit_findings;

// Re-export the rig Tool wrapper types
pub use custom_command::CustomCommandTool;
pub use glob::GlobTool;
pub use list_directory::ListDirectoryTool;
pub use read_file::ReadFileTool;
pub use read_files::ReadFilesTool;
pub use search_text::SearchTextTool;
pub use submit_findings::{SUBMIT_FINDINGS_TOOL_NAME, SubmitFindingsTool};

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

/// Convenience helper: time a tool call and record the result.
///
/// Returns the start `Instant` — call [`finish_tool_call`] with it
/// after the call completes.
pub fn start_tool_call() -> Instant {
    Instant::now()
}

/// Complete a tool call recording.
///
/// Forwards the entry to [`crate::audit::record`], which appends it
/// to the current task's audit buffer when an audit scope is active.
/// Outside a scope this is a no-op.
pub fn finish_tool_call(
    start: Instant,
    tool_name: &str,
    args_summary: impl Into<String>,
    result_summary: impl Into<String>,
) {
    crate::audit::record(&ToolCallEntry {
        tool_name: tool_name.to_string(),
        args_summary: args_summary.into(),
        result_summary: result_summary.into(),
        duration: start.elapsed(),
    });
}
