//! Per-run audit log artifact.
//!
//! # Bounded Context: Run Audit
//!
//! Captures a structured, post-hoc record of *what actually happened*
//! during a review run — which agents ran against which files, how
//! many turns the agent loop took, which tools the LLM invoked, how
//! many tokens each task consumed, what the critic dropped, and the
//! final findings. The artifact is meant to be saved as a CI build
//! artifact for after-the-fact inspection.
//!
//! The audit log is opt-in via the `--audit-log <PATH>` CLI flag,
//! the `NITPIK_AUDIT_LOG` env var, or the `[review].audit_log` key
//! in `.nitpik.toml`. When opt-in is detected, the orchestrator
//! collects per-task data through a `tokio::task_local` channel; when
//! the flag is unset the entire pipeline runs unchanged with zero
//! overhead.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;
use tokio::task_local;

use crate::models::TokenUsage;
use crate::models::finding::Finding;
use crate::orchestrator::verify::DroppedFinding;
use crate::tools::ToolCallEntry;

/// Top-level audit document written to disk.
#[derive(Debug, Clone, Serialize)]
pub struct RunAudit {
    /// Schema version. Bump when the document layout changes in a
    /// way that breaks existing readers.
    pub schema: u32,
    /// Random per-run identifier (UUID v4) for log correlation.
    pub run_id: String,
    /// Wall-clock start time as Unix epoch milliseconds.
    pub started_at_unix_ms: u64,
    /// Wall-clock duration of the review run.
    pub duration_ms: u64,
    /// Static configuration snapshot (no secrets).
    pub config: ConfigSummary,
    /// Per file × agent task records.
    pub tasks: Vec<TaskAudit>,
    /// Critic verify pass outcome (only present when `--verify`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyAudit>,
    /// Final, deduplicated, severity-sorted findings the run produced.
    pub findings: Vec<Finding>,
    /// Number of file × agent tasks that failed after retries.
    pub failed_tasks: usize,
    /// Aggregate token usage across all tasks.
    pub tokens: TokenUsage,
}

/// Static configuration snapshot. Secrets are explicitly excluded.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSummary {
    pub provider: String,
    pub model: String,
    pub agentic: bool,
    pub max_turns: usize,
    pub max_tool_calls: usize,
    pub max_concurrent: usize,
    pub profiles: Vec<String>,
    pub multi_wave: bool,
    pub verify: bool,
    pub auto_mode: Option<String>,
    pub review_scope: String,
    pub nitpik_version: String,
    /// Per-attempt timeout in seconds for each file × agent review
    /// call. `0` means timeouts were disabled.
    pub timeout_secs: u64,
}

/// Audit data for a single file × agent task.
#[derive(Debug, Clone, Serialize)]
pub struct TaskAudit {
    pub agent: String,
    pub file: String,
    pub model: String,
    pub wave: u8,
    pub status: TaskStatus,
    /// Short error classification when `status == Failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Number of retry attempts the orchestrator performed
    /// (0 = succeeded on first attempt).
    pub retries: usize,
    pub tokens: TokenUsage,
    /// Tool calls the LLM made inside this task's agent loop, in
    /// invocation order. Empty when agentic mode was off or the
    /// task was a cache hit.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Number of findings the agent emitted (before deduplication).
    pub findings_emitted: usize,
    /// Number of completion calls (turns) on the successful attempt.
    /// `0` for non-agentic calls and cache hits.
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub turns: usize,
    /// Set when the loop terminated because the model invoked the
    /// terminal tool (typically `submit_findings`). When `None` for
    /// an agentic task the model emitted prose without calling the
    /// terminal tool — usually a model-quality issue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminated_via_tool: Option<String>,
    /// True when the loop fired a self-repair correction because the
    /// model returned text without calling the terminal tool. A
    /// frequent self-repair rate signals the chosen model handles
    /// the structured-output contract poorly.
    #[serde(skip_serializing_if = "is_false")]
    pub self_repair_attempted: bool,
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// Final terminal state of a review task.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Done,
    CacheHit,
    Failed,
}

/// One tool invocation record.
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallRecord {
    pub tool: String,
    pub args_summary: String,
    pub result_summary: String,
    pub duration_ms: u64,
}

impl From<ToolCallEntry> for ToolCallRecord {
    fn from(e: ToolCallEntry) -> Self {
        Self {
            tool: e.tool_name,
            args_summary: e.args_summary,
            result_summary: e.result_summary,
            duration_ms: e.duration.as_millis() as u64,
        }
    }
}

/// Critic verify pass outcome.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyAudit {
    pub kept: usize,
    pub dropped: Vec<DroppedRecord>,
    pub tokens: TokenUsage,
}

/// One critic-dropped finding plus the critic's stated reason.
#[derive(Debug, Clone, Serialize)]
pub struct DroppedRecord {
    pub agent: String,
    pub file: String,
    pub line: u32,
    pub title: String,
    pub reason: String,
}

impl From<DroppedFinding> for DroppedRecord {
    fn from(d: DroppedFinding) -> Self {
        Self {
            agent: d.finding.agent,
            file: d.finding.file,
            line: d.finding.line,
            title: d.finding.title,
            reason: d.reason,
        }
    }
}

impl RunAudit {
    /// Serialize to pretty JSON.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Write the audit document to `path` as pretty JSON.
    ///
    /// The parent directory is created if it does not already exist.
    pub fn write_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let json = self
            .to_json_string()
            .map_err(|e| std::io::Error::other(format!("audit log serialization failed: {e}")))?;
        std::fs::write(path, json)
    }
}

/// Current Unix epoch milliseconds.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ─── Per-task tool-call collection ────────────────────────────────

/// Shared, mutable buffer of tool-call entries for a single task.
pub type TaskToolBuffer = Arc<Mutex<Vec<ToolCallEntry>>>;

task_local! {
    static TASK_BUFFER: Option<TaskToolBuffer>;
}

/// Run `f` with `buffer` installed as the current task's tool-call
/// sink. Tool calls recorded inside `f` (via [`record`]) are appended
/// to `buffer` in invocation order. Outside any scope, [`record`] is
/// a no-op for audit purposes.
///
/// Tool calls are still pushed into the global `ToolCallLog` queue
/// for the live progress display — the per-task buffer is purely
/// additive.
pub async fn scope<F: std::future::Future>(buffer: Option<TaskToolBuffer>, f: F) -> F::Output {
    TASK_BUFFER.scope(buffer, f).await
}

/// Append `entry` to the current task's tool buffer if a scope is
/// active. No-op otherwise.
pub fn record(entry: &ToolCallEntry) {
    let _ = TASK_BUFFER.try_with(|buf| {
        if let Some(buf) = buf.as_ref() {
            if let Ok(mut guard) = buf.lock() {
                guard.push(entry.clone());
            }
        }
    });
}

/// Create an empty per-task buffer.
pub fn new_buffer() -> TaskToolBuffer {
    Arc::new(Mutex::new(Vec::new()))
}

/// Drain `buffer` into a `Vec<ToolCallRecord>`.
pub fn drain_records(buffer: &TaskToolBuffer) -> Vec<ToolCallRecord> {
    let entries = match buffer.lock() {
        Ok(mut g) => std::mem::take(&mut *g),
        Err(_) => Vec::new(),
    };
    entries.into_iter().map(ToolCallRecord::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn entry(name: &str) -> ToolCallEntry {
        ToolCallEntry {
            tool_name: name.into(),
            args_summary: "args".into(),
            result_summary: "ok".into(),
            duration: Duration::from_millis(7),
        }
    }

    #[tokio::test]
    async fn record_outside_scope_is_noop() {
        // Must not panic.
        record(&entry("read_file"));
    }

    #[tokio::test]
    async fn record_inside_scope_collects_entries() {
        let buf = new_buffer();
        scope(Some(Arc::clone(&buf)), async {
            record(&entry("read_file"));
            record(&entry("search_text"));
        })
        .await;
        let records = drain_records(&buf);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tool, "read_file");
        assert_eq!(records[1].tool, "search_text");
        assert_eq!(records[0].duration_ms, 7);
    }

    #[tokio::test]
    async fn nested_scopes_shadow_outer() {
        let outer = new_buffer();
        let inner = new_buffer();
        scope(Some(Arc::clone(&outer)), async {
            record(&entry("a"));
            scope(Some(Arc::clone(&inner)), async {
                record(&entry("b"));
            })
            .await;
            record(&entry("c"));
        })
        .await;
        let outer_recs = drain_records(&outer);
        let inner_recs = drain_records(&inner);
        assert_eq!(outer_recs.len(), 2);
        assert_eq!(outer_recs[0].tool, "a");
        assert_eq!(outer_recs[1].tool, "c");
        assert_eq!(inner_recs.len(), 1);
        assert_eq!(inner_recs[0].tool, "b");
    }

    #[test]
    fn run_audit_serializes_to_json() {
        let audit = RunAudit {
            schema: 1,
            run_id: "test-id".into(),
            started_at_unix_ms: 1_700_000_000_000,
            duration_ms: 1234,
            config: ConfigSummary {
                provider: "anthropic".into(),
                model: "claude-x".into(),
                agentic: true,
                max_turns: 10,
                max_tool_calls: 10,
                max_concurrent: 5,
                profiles: vec!["backend".into()],
                multi_wave: false,
                verify: false,
                auto_mode: None,
                review_scope: "main".into(),
                nitpik_version: "0.0.0".into(),
                timeout_secs: 0,
            },
            tasks: vec![],
            verify: None,
            findings: vec![],
            failed_tasks: 0,
            tokens: TokenUsage::default(),
        };
        let json = audit.to_json_string().unwrap();
        assert!(json.contains("\"run_id\": \"test-id\""));
        assert!(json.contains("\"schema\": 1"));
        // Top-level `verify` object omitted when None (the
        // ConfigSummary still has a boolean `verify` field, so we
        // assert on the nested critic-only fields instead).
        assert!(!json.contains("\"kept\":"));
        assert!(!json.contains("\"dropped\":"));
    }

    #[test]
    fn run_audit_writes_to_disk_creating_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/sub/audit.json");
        let audit = RunAudit {
            schema: 1,
            run_id: "rid".into(),
            started_at_unix_ms: 0,
            duration_ms: 0,
            config: ConfigSummary {
                provider: "anthropic".into(),
                model: "m".into(),
                agentic: false,
                max_turns: 0,
                max_tool_calls: 0,
                max_concurrent: 1,
                profiles: vec![],
                multi_wave: false,
                verify: false,
                auto_mode: None,
                review_scope: "".into(),
                nitpik_version: "0.0.0".into(),
                timeout_secs: 0,
            },
            tasks: vec![],
            verify: None,
            findings: vec![],
            failed_tasks: 0,
            tokens: TokenUsage::default(),
        };
        audit.write_to(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("\"run_id\": \"rid\""));
    }
}
