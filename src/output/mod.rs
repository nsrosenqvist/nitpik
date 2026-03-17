//! Output formatters: terminal, JSON, GitHub Actions, GitLab Code Quality, Bitbucket, Checkstyle, Forgejo.
//!
//! # Bounded Context: Rendering
//!
//! Owns the `OutputRenderer` trait and all format implementations.
//! Consumes `Vec<Finding>` and produces formatted output strings —
//! has no knowledge of LLM providers, diffs, or orchestration.

pub mod bitbucket;
pub mod checkstyle;
pub mod escape;
pub mod forgejo;
pub mod github;
pub mod gitlab;
pub mod json;
pub mod terminal;

use crate::models::finding::Finding;

/// Trait for formatting review findings to an output string (sync, pure).
pub trait OutputFormatter {
    /// Format findings to a string.
    fn format(&self, findings: &[Finding]) -> String;
}

/// Trait for publishing review findings to external services (async, side-effecting).
pub trait OutputPublisher {
    /// Publish findings to an external service.
    fn publish(
        &self,
        findings: &[Finding],
    ) -> impl std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send;
}
