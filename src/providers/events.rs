//! Live event stream for the agent loop.
//!
//! The agent loop emits `LoopEvent` values at turn and tool-call
//! boundaries through a [`tokio::task_local`] sink. The orchestrator
//! installs a sink before invoking the provider so it can route those
//! events to the live progress display; observers in tests can pin a
//! sink that records into a `Vec`.
//!
//! The sink is optional: when no scope is active the emitter is a
//! no-op, so the loop runs unchanged for callers that do not opt in.

use std::sync::Arc;
use std::time::Duration;

use tokio::task_local;

/// One observable boundary inside the agent loop.
///
/// Variants are grouped per turn — every `TurnStart` is matched by a
/// `TurnEnd`, and `ToolCallStart`/`ToolCallEnd` always come in pairs
/// nested inside the same turn.
#[derive(Debug, Clone)]
pub enum LoopEvent {
    /// Emitted right before the n-th completion call (1-indexed).
    TurnStart { turn: usize },
    /// Emitted before the loop dispatches a tool call to the registry.
    /// `args_summary` is a compact one-line preview of the arguments
    /// (truncated as needed) suitable for showing on a spinner.
    ToolCallStart { tool: String, args_summary: String },
    /// Emitted after the tool returns, with the wall-clock duration
    /// and a flag indicating success vs. error.
    ToolCallEnd {
        tool: String,
        ok: bool,
        duration: Duration,
    },
    /// Emitted after the assistant turn completes, carrying the per-turn
    /// token deltas reported by the provider.
    TurnEnd {
        turn: usize,
        input_tokens: u64,
        output_tokens: u64,
    },
}

/// Sink for `LoopEvent`s. Implementors must be cheap to clone and
/// safe to call from inside async work (no blocking).
pub type LoopEventSink = Arc<dyn Fn(LoopEvent) + Send + Sync>;

task_local! {
    static EVENT_SINK: Option<LoopEventSink>;
}

/// Run `f` with `sink` installed as the current task's event sink.
///
/// Nested scopes shadow the outer sink for the duration of `f`. A
/// `None` sink disables emission inside the scope.
pub async fn scope<F: std::future::Future>(sink: Option<LoopEventSink>, f: F) -> F::Output {
    EVENT_SINK.scope(sink, f).await
}

/// Emit `event` to the current scope's sink, if any. Outside a scope
/// (or with `None` installed) this is a no-op.
pub fn emit(event: LoopEvent) {
    let _ = EVENT_SINK.try_with(|sink| {
        if let Some(s) = sink.as_ref() {
            s(event);
        }
    });
}

/// Compact a JSON args object into a single-line preview for display.
///
/// Strips whitespace and truncates to `max_len` chars with an
/// ellipsis. Best-effort — the input is whatever `serde_json`
/// produced for the tool's arguments.
pub fn summarize_args(args_json: &str, max_len: usize) -> String {
    let compact: String = args_json.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_len {
        compact
    } else {
        let mut out: String = compact.chars().take(max_len.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn collector() -> (LoopEventSink, Arc<Mutex<Vec<LoopEvent>>>) {
        let log: Arc<Mutex<Vec<LoopEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let log_clone = Arc::clone(&log);
        let sink: LoopEventSink = Arc::new(move |ev| {
            log_clone.lock().unwrap().push(ev);
        });
        (sink, log)
    }

    #[tokio::test]
    async fn emit_outside_scope_is_noop() {
        // Should not panic.
        emit(LoopEvent::TurnStart { turn: 1 });
    }

    #[tokio::test]
    async fn emit_inside_scope_reaches_sink() {
        let (sink, log) = collector();
        scope(Some(sink), async {
            emit(LoopEvent::TurnStart { turn: 1 });
            emit(LoopEvent::TurnEnd {
                turn: 1,
                input_tokens: 10,
                output_tokens: 5,
            });
        })
        .await;
        let recorded = log.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        matches!(recorded[0], LoopEvent::TurnStart { turn: 1 });
    }

    #[tokio::test]
    async fn none_sink_disables_emission() {
        scope(None, async {
            emit(LoopEvent::TurnStart { turn: 99 });
        })
        .await;
        // No sink, no observable side effect — just confirm we didn't panic.
    }

    #[test]
    fn summarize_args_passes_short_payloads_through() {
        assert_eq!(
            summarize_args(r#"{"path":"src/x.rs"}"#, 40),
            r#"{"path":"src/x.rs"}"#
        );
    }

    #[test]
    fn summarize_args_truncates_long_payloads() {
        let long = r#"{"path":"a-very-long-path-that-exceeds-the-budget"}"#;
        let s = summarize_args(long, 20);
        assert!(s.chars().count() <= 20);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn summarize_args_collapses_whitespace() {
        let s = summarize_args("{\n  \"path\":\n  \"x\"\n}", 40);
        assert_eq!(s, r#"{ "path": "x" }"#);
    }
}
