//! ReviewProvider trait and LLM integration.
//!
//! # Bounded Context: LLM Providers
//!
//! Owns the `ReviewProvider` trait, rig-core client construction,
//! prompt dispatch, and response parsing. Abstracts the 19-provider
//! matrix behind a single `review()` call — callers never touch
//! rig-core types directly.

pub mod response;
pub mod rig;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::AgentDefinition;
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
    /// Free-form rationale; included to mirror the prompt schema but
    /// currently unused by callers.
    #[serde(default)]
    pub rationale: Option<String>,
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
    ) -> Result<Vec<Finding>, ProviderError>;

    /// Classify threat findings via a single-turn structured-output call.
    ///
    /// Used by the threat scanner's triage step to reclassify pattern
    /// matches as confirmed, dismissed, or downgraded.
    async fn triage(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<Vec<TriageVerdict>, ProviderError>;
}
