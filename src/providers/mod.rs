//! ReviewProvider trait and LLM integration.
//!
//! # Bounded Context: LLM Providers
//!
//! Owns the `ReviewProvider` trait, rig-core client construction,
//! prompt dispatch, and response parsing. Abstracts the 19-provider
//! matrix behind a single `review()` call — callers never touch
//! rig-core types directly.

pub mod agent_loop;
pub mod events;
pub mod response;
pub mod rig;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::AgentDefinition;
use crate::models::TokenUsage;
use crate::models::finding::Finding;

/// Errors from the review provider.
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("LLM API error: {0}")]
    ApiError(String),

    #[error("failed to parse LLM response: {0}")]
    ParseError(String),

    #[error("provider not configured: {0}")]
    NotConfigured(String),
}

/// JSON-schema wrapper around `Vec<Finding>` for structured-output mode.
///
/// OpenAI's structured-output spec requires the response_format schema's
/// root to be `type: "object"`. The real OpenAI API is lenient and
/// accepts a top-level `type: "array"` schema, but stricter compatible
/// endpoints (GitHub Models, future Azure structured mode) reject it
/// with a 400. Passing this wrapper as the schema type yields a
/// spec-compliant object schema; [`response::parse_findings_response`]
/// already accepts both bare-array and wrapped responses.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FindingsResponse {
    pub findings: Vec<Finding>,
}

/// JSON-schema wrapper around `Vec<TriageVerdict>` for structured-output
/// mode. See [`FindingsResponse`] for the rationale.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct VerdictsResponse {
    pub verdicts: Vec<TriageVerdict>,
}

/// Raw triage verdict produced by the LLM for a single threat finding.
///
/// The classification string is kept as-is so the providers layer does
/// not depend on the threat module's enum; the consumer normalizes it.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TriageVerdict {
    /// Index of the finding in the original list (0-based).
    pub index: usize,
    /// One of "confirmed", "dismissed", or "downgraded".
    pub classification: String,
    /// Free-form rationale. Surfaced to the audit log when the critic
    /// drops a finding so users can see why.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// Outcome of a single `ReviewProvider::review` call: the parsed
/// findings plus the token usage consumed by the underlying LLM
/// round trips (one round in non-agentic mode, multiple in
/// agentic mode).
#[derive(Debug, Clone, Default)]
pub struct ReviewOutcome {
    /// Findings the LLM produced (may be empty).
    pub findings: Vec<Finding>,
    /// Token usage aggregated across every LLM round.
    pub tokens: TokenUsage,
    /// Diagnostic flags from the agent loop. Default (all-zero) for
    /// non-agentic calls; agentic calls fill these so the audit log
    /// can show how the model behaved.
    pub diagnostics: AgentDiagnostics,
}

/// Per-call agent-loop diagnostics, surfaced to the audit log.
///
/// All fields are zero/`None`/`false` for non-agentic calls.
#[derive(Debug, Clone, Default)]
pub struct AgentDiagnostics {
    /// Number of completion calls made on this attempt (one per turn).
    pub turns: usize,
    /// Set when the loop terminated because the model invoked the
    /// configured terminal tool (typically `submit_findings`). When
    /// `None` for an agentic call, the model emitted prose without
    /// calling the terminal tool — usually a model-quality issue.
    pub terminated_via_tool: Option<String>,
    /// True when the loop appended a self-repair correction message
    /// because the model returned text without calling the terminal
    /// tool. Frequent self-repairs indicate the chosen model handles
    /// the structured-output contract poorly.
    pub self_repair_attempted: bool,
}

/// Outcome of a single `ReviewProvider::triage` call.
#[derive(Debug, Clone, Default)]
pub struct TriageOutcome {
    /// Verdicts the LLM produced (may be empty).
    pub verdicts: Vec<TriageVerdict>,
    /// Token usage for the triage call.
    pub tokens: TokenUsage,
}

/// Trait for LLM-backed code review.
///
/// Implementations handle agent construction, prompt building,
/// and response parsing.
#[async_trait]
pub trait ReviewProvider: Send + Sync {
    /// Perform a code review and return findings.
    ///
    /// When `agentic` is true, `max_turns` and `max_tool_calls` control
    /// the budget for the agentic exploration loop.
    async fn review(
        &self,
        agent: &AgentDefinition,
        prompt: &str,
        agentic: bool,
        max_turns: usize,
        max_tool_calls: usize,
    ) -> Result<ReviewOutcome, ProviderError>;

    /// Classify threat findings via a single-turn structured-output call.
    ///
    /// Used by the threat scanner's triage step to reclassify pattern
    /// matches as confirmed, dismissed, or downgraded.
    async fn triage(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<TriageOutcome, ProviderError>;
}
