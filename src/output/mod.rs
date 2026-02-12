//! Output renderers: terminal, JSON, GitHub Actions, GitLab Code Quality, Bitbucket, Forgejo.

pub mod bitbucket;
pub mod forgejo;
pub mod github;
pub mod gitlab;
pub mod json;
pub mod terminal;

use crate::models::finding::Finding;

/// Trait for rendering review findings to an output format.
pub trait OutputRenderer {
    /// Render findings to a string.
    fn render(&self, findings: &[Finding]) -> String;
}
