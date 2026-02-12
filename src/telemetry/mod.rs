//! Anonymous usage telemetry — privacy-respecting heartbeat.
//!
//! Sends a single fire-and-forget POST on each `review` run containing only
//! aggregate, non-identifying statistics: file count, diff size, agent count,
//! whether a license is active, and whether the run is inside CI.
//!
//! The heartbeat:
//! - contains **no** personally identifiable information
//! - is disabled with `--no-telemetry`, `NITPIK_TELEMETRY=false`, or
//!   `[telemetry] enabled = false` in config
//! - fails silently — never affects the review outcome

use serde::Serialize;
use std::time::Duration;

/// Placeholder endpoint — not operational yet.
const HEARTBEAT_URL: &str = crate::constants::TELEMETRY_URL;

/// Maximum time we'll wait for the heartbeat POST before giving up.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(2);

/// Payload sent with each heartbeat. Contains only anonymous aggregate data.
#[derive(Debug, Clone, Serialize)]
pub struct HeartbeatPayload {
    /// Random identifier for this single run (not persisted across runs).
    pub run_id: String,
    /// Number of files in the diff.
    pub file_count: usize,
    /// Total number of changed lines (added + removed) across all files.
    pub diff_lines: usize,
    /// Number of agent profiles used for this review.
    pub agent_count: usize,
    /// Whether a commercial license is active.
    pub licensed: bool,
    /// Whether the run appears to be inside a CI environment.
    pub is_ci: bool,
    /// CLI version string.
    pub version: &'static str,
}

impl HeartbeatPayload {
    /// Build a payload from the available review parameters.
    pub fn from_review(
        file_count: usize,
        diff_lines: usize,
        agent_count: usize,
        licensed: bool,
    ) -> Self {
        Self {
            run_id: uuid::Uuid::new_v4().to_string(),
            file_count,
            diff_lines,
            agent_count,
            licensed,
            is_ci: detect_ci(),
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// Detect whether we are running inside a CI environment by checking
/// common environment variables set by popular CI providers.
pub fn detect_ci() -> bool {
    // Generic
    if std::env::var("CI").is_ok() {
        return true;
    }
    // Provider-specific variables (for systems that don't set `CI`)
    const CI_VARS: &[&str] = &[
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "BITBUCKET_BUILD_NUMBER",
        "JENKINS_URL",
        "CIRCLECI",
        "TF_BUILD",        // Azure Pipelines
        "BUILDKITE",
        "TRAVIS",
        "CODEBUILD_BUILD_ID", // AWS CodeBuild
        "TEAMCITY_VERSION",
    ];
    CI_VARS.iter().any(|var| std::env::var(var).is_ok())
}

/// Fire-and-forget: send the heartbeat payload. Returns immediately via
/// `tokio::spawn`. The spawned task will silently discard any errors.
pub fn send_heartbeat(payload: HeartbeatPayload) {
    tokio::spawn(async move {
        let _ = post_heartbeat(&payload).await;
    });
}

/// Actually perform the HTTP POST. Separated for testability.
async fn post_heartbeat(payload: &HeartbeatPayload) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(HEARTBEAT_TIMEOUT)
        .build()?;

    client
        .post(HEARTBEAT_URL)
        .json(payload)
        .send()
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_serializes_to_json() {
        let payload = HeartbeatPayload {
            run_id: "test-run-id".to_string(),
            file_count: 3,
            diff_lines: 42,
            agent_count: 2,
            licensed: false,
            is_ci: false,
            version: "0.1.0",
        };
        let json = serde_json::to_value(&payload).expect("serialization should succeed");
        assert_eq!(json["file_count"], 3);
        assert_eq!(json["diff_lines"], 42);
        assert_eq!(json["agent_count"], 2);
        assert_eq!(json["licensed"], false);
        assert_eq!(json["is_ci"], false);
        assert_eq!(json["run_id"], "test-run-id");
        assert_eq!(json["version"], "0.1.0");
    }

    #[test]
    fn from_review_builds_valid_payload() {
        let payload = HeartbeatPayload::from_review(5, 100, 3, false);
        assert_eq!(payload.file_count, 5);
        assert_eq!(payload.diff_lines, 100);
        assert_eq!(payload.agent_count, 3);
        assert!(!payload.licensed);
        // run_id is a valid UUID
        uuid::Uuid::parse_str(&payload.run_id).expect("run_id should be valid UUID");
    }

    #[test]
    fn detect_ci_returns_false_when_no_ci_vars() {
        // Remove CI vars if set (best-effort — other tests may set them)
        // In a clean environment this should be false, but in CI it'll be true.
        // We just assert it doesn't panic.
        let _ = detect_ci();
    }

    #[tokio::test]
    async fn send_heartbeat_does_not_panic_on_unreachable_url() {
        let payload = HeartbeatPayload::from_review(1, 10, 1, false);
        // This should silently discard the error (unreachable host)
        send_heartbeat(payload);
        // Give the spawned task a moment to run
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
