//! Per-task tool-call budget enforcement.
//!
//! Each agentic review task scopes a [`ToolBudget`] via a tokio
//! task-local so that *every* tool invocation — built-in or custom —
//! is counted toward a single hard cap. When the cap is exhausted,
//! [`try_consume`] returns an error message that is fed back to the LLM
//! as the tool's failure result. The LLM sees:
//!
//! ```text
//! tool budget exhausted: <N> tool calls already made (max <M>);
//! finalize your findings now and return the JSON response.
//! ```
//!
//! ...which (in practice) terminates the exploration phase and forces
//! the model to commit to its current understanding.
//!
//! Tasks that don't set up a budget (most unit tests, the non-agentic
//! review path) see [`try_consume`] succeed indefinitely.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

tokio::task_local! {
    /// Active tool-call budget for the current async task.
    static TOOL_BUDGET: Arc<ToolBudget>;
}

/// Counts tool invocations within a single task scope and rejects
/// further calls once the configured maximum is reached.
#[derive(Debug)]
pub struct ToolBudget {
    /// Number of tool calls used so far.
    used: AtomicUsize,
    /// Maximum allowed calls (`0` means "unlimited", treated as a
    /// safety release valve when callers explicitly opt out).
    max: usize,
}

impl ToolBudget {
    /// Create a new budget. `max == 0` disables enforcement.
    pub fn new(max: usize) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicUsize::new(0),
            max,
        })
    }

    /// Number of calls already counted toward the budget. Test-only
    /// observability; production code reads neither used nor max.
    #[cfg(test)]
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    /// Maximum the budget allows. `0` = unlimited. Test-only.
    #[cfg(test)]
    pub fn max(&self) -> usize {
        self.max
    }

    /// Try to record one more invocation. Returns `Err` when the budget
    /// is exhausted; the error message is the exact text fed back to the
    /// LLM as the tool result.
    fn consume(&self, tool_name: &str) -> Result<(), String> {
        if self.max == 0 {
            // Unlimited.
            self.used.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }

        let prior = self.used.fetch_add(1, Ordering::Relaxed);
        if prior >= self.max {
            // Roll back so `used()` reflects the cap precisely
            // (saturating semantics for observers).
            self.used.fetch_sub(1, Ordering::Relaxed);
            return Err(format!(
                "tool budget exhausted: {} tool calls already made (max {}); \
                 finalize your findings now and return the JSON response. \
                 The `{tool_name}` call was not executed.",
                self.max, self.max,
            ));
        }
        Ok(())
    }
}

/// Run `fut` with `budget` installed as the active task-local budget.
///
/// Inside the future, every [`try_consume`] call sees this budget; once
/// the future resolves the task-local goes out of scope and tools called
/// outside fall back to "unlimited" (their default).
pub async fn scope<F, T>(budget: Arc<ToolBudget>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    TOOL_BUDGET.scope(budget, fut).await
}

/// Attempt to consume one budget unit on behalf of `tool_name`.
///
/// Returns `Ok(())` when the call is allowed, `Err(message)` when the
/// budget is exhausted. Tools should propagate the message verbatim
/// to the LLM so the model can react.
///
/// When no budget is installed (no [`scope`] wrapper) the call is
/// always permitted. This keeps tests and non-agentic flows working.
pub fn try_consume(tool_name: &str) -> Result<(), String> {
    TOOL_BUDGET
        .try_with(|b| b.consume(tool_name))
        .unwrap_or(Ok(()))
}

/// Snapshot of the active budget (used / max), or `None` when no
/// budget is installed. Test-only observability — production code
/// reads token usage from the audit log instead.
#[cfg(test)]
pub fn snapshot() -> Option<(usize, usize)> {
    TOOL_BUDGET.try_with(|b| (b.used(), b.max())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_when_no_budget_set() {
        for _ in 0..1000 {
            assert!(try_consume("read_file").is_ok());
        }
        assert!(snapshot().is_none());
    }

    #[tokio::test]
    async fn budget_zero_means_unlimited() {
        let budget = ToolBudget::new(0);
        scope(budget, async {
            for _ in 0..50 {
                assert!(try_consume("read_file").is_ok());
            }
            let (used, max) = snapshot().unwrap();
            assert_eq!(used, 50);
            assert_eq!(max, 0);
        })
        .await;
    }

    #[tokio::test]
    async fn budget_blocks_after_max() {
        let budget = ToolBudget::new(3);
        scope(budget, async {
            assert!(try_consume("read_file").is_ok());
            assert!(try_consume("read_file").is_ok());
            assert!(try_consume("read_file").is_ok());

            // 4th call rejected.
            let err = try_consume("read_file").unwrap_err();
            assert!(err.contains("tool budget exhausted"));
            assert!(err.contains("max 3"));
            assert!(err.contains("read_file"));

            // Subsequent calls still rejected; counter stays at max.
            assert!(try_consume("search_text").is_err());

            let (used, max) = snapshot().unwrap();
            assert_eq!(used, max);
            assert_eq!(max, 3);
        })
        .await;
    }

    #[tokio::test]
    async fn budgets_are_task_local() {
        let outer = ToolBudget::new(2);
        scope(outer, async {
            assert!(try_consume("read_file").is_ok());

            // A nested task with its own budget doesn't leak counts back.
            let inner = ToolBudget::new(5);
            scope(inner, async {
                for _ in 0..5 {
                    assert!(try_consume("search_text").is_ok());
                }
                assert!(try_consume("search_text").is_err());
            })
            .await;

            // Outer budget still has 1 unit left.
            assert!(try_consume("read_file").is_ok());
            assert!(try_consume("read_file").is_err());
        })
        .await;
    }

    #[tokio::test]
    async fn budget_message_mentions_tool_name() {
        let budget = ToolBudget::new(1);
        scope(budget, async {
            assert!(try_consume("anything").is_ok());
            let err = try_consume("custom_run_tests").unwrap_err();
            assert!(err.contains("custom_run_tests"));
        })
        .await;
    }
}
