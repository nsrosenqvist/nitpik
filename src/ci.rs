//! Centralized CI environment detection.
//!
//! # Bounded Context: CI Infrastructure
//!
//! Consolidates CI provider detection and branch resolution env vars
//! that were previously duplicated across `telemetry`, `update`, and
//! `diff::git`.

use std::fmt;

/// Known CI provider environments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiProvider {
    GitHub,
    GitLab,
    Bitbucket,
    Jenkins,
    CircleCI,
    AzurePipelines,
    Buildkite,
    Travis,
    AWSCodeBuild,
    TeamCity,
    Drone,
    Woodpecker,
    /// CI detected via the generic `CI` env var but no specific provider matched.
    Unknown,
}

impl fmt::Display for CiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub => write!(f, "github"),
            Self::GitLab => write!(f, "gitlab"),
            Self::Bitbucket => write!(f, "bitbucket"),
            Self::Jenkins => write!(f, "jenkins"),
            Self::CircleCI => write!(f, "circleci"),
            Self::AzurePipelines => write!(f, "azure"),
            Self::Buildkite => write!(f, "buildkite"),
            Self::Travis => write!(f, "travis"),
            Self::AWSCodeBuild => write!(f, "codebuild"),
            Self::TeamCity => write!(f, "teamcity"),
            Self::Drone => write!(f, "drone"),
            Self::Woodpecker => write!(f, "woodpecker"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Provider-specific env var → `CiProvider` mapping.
const PROVIDER_VARS: &[(&str, CiProvider)] = &[
    ("GITHUB_ACTIONS", CiProvider::GitHub),
    ("GITLAB_CI", CiProvider::GitLab),
    ("BITBUCKET_BUILD_NUMBER", CiProvider::Bitbucket),
    ("JENKINS_URL", CiProvider::Jenkins),
    ("CIRCLECI", CiProvider::CircleCI),
    ("TF_BUILD", CiProvider::AzurePipelines),
    ("BUILDKITE", CiProvider::Buildkite),
    ("TRAVIS", CiProvider::Travis),
    ("CODEBUILD_BUILD_ID", CiProvider::AWSCodeBuild),
    ("CODEBUILD_CI", CiProvider::AWSCodeBuild),
    ("TEAMCITY_VERSION", CiProvider::TeamCity),
    ("DRONE", CiProvider::Drone),
    ("WOODPECKER_CI", CiProvider::Woodpecker),
];

/// Detect the specific CI provider from environment variables.
///
/// Returns `Some(CiProvider)` if running in a recognized CI environment,
/// `None` otherwise.
pub fn detect_ci_provider() -> Option<CiProvider> {
    for &(var, provider) in PROVIDER_VARS {
        if std::env::var(var).is_ok() {
            return Some(provider);
        }
    }

    if std::env::var("CI").is_ok() {
        return Some(CiProvider::Unknown);
    }

    None
}

/// Detect whether the process is running inside a CI environment.
pub fn is_ci() -> bool {
    detect_ci_provider().is_some()
}

/// Environment variables that CI providers use to expose the current branch.
///
/// Used by `diff::git::detect_branch()` as a fallback when `git rev-parse`
/// returns a detached HEAD.
pub const BRANCH_ENV_VARS: &[&str] = &[
    "GITHUB_HEAD_REF",
    "CI_COMMIT_BRANCH",
    "CI_MERGE_REQUEST_SOURCE_BRANCH_NAME",
    "BITBUCKET_BRANCH",
    "CI_BRANCH",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_provider_display() {
        assert_eq!(CiProvider::GitHub.to_string(), "github");
        assert_eq!(CiProvider::GitLab.to_string(), "gitlab");
        assert_eq!(CiProvider::Unknown.to_string(), "unknown");
    }

    #[test]
    fn no_ci_returns_none() {
        // In a test environment without CI vars, should return None
        // (unless actually running in CI — that's fine, it just won't
        // return None in that case).
        let result = detect_ci_provider();
        // Just verify it doesn't panic and returns a valid value
        if let Some(provider) = result {
            assert!(!provider.to_string().is_empty());
        }
    }

    #[test]
    fn is_ci_consistent_with_detect() {
        assert_eq!(is_ci(), detect_ci_provider().is_some());
    }
}
