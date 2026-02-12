//! ReviewProvider trait and LLM integration.
//!
//! Provides an abstraction layer over rig-core to decouple the
//! codebase from the specific LLM library.

pub mod rig;

use async_trait::async_trait;
use thiserror::Error;

use crate::models::finding::Finding;
use crate::models::AgentDefinition;

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
}
