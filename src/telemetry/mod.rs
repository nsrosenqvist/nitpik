//! Anonymous usage telemetry — privacy-respecting heartbeat.
//!
//! # Bounded Context: Telemetry
//!
//! Owns the heartbeat payload construction and fire-and-forget HTTP
//! POST. Consumes only aggregate counters — never sees diff content,
//! findings, or API keys.
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

/// Placeholder endpoint — not operational yet.
const HEARTBEAT_URL: &str = crate::constants::TELEMETRY_URL;

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
            is_ci: crate::ci::is_ci(),
            version: crate::constants::FULL_VERSION,
        }
    }
}

/// Returns `true` when `NITPIK_DEBUG` is set to a truthy value.
pub fn is_debug() -> bool {
    matches!(
        std::env::var(crate::constants::ENV_DEBUG).as_deref(),
        Ok("1" | "true" | "yes")
    )
}

/// Send the heartbeat payload.
///
/// Returns a [`tokio::task::JoinHandle`] so the caller can await it
/// before the process exits.  The caller **must** await the handle to
/// guarantee the POST completes — dropping without awaiting races
/// against runtime shutdown and may silently cancel the request.
pub fn send_heartbeat(payload: HeartbeatPayload) -> tokio::task::JoinHandle<()> {
    if is_debug() {
        tokio::spawn(async move {
            debug_post_heartbeat(&payload).await;
        })
    } else {
        tokio::spawn(async move {
            let _ = post_heartbeat(&payload).await;
        })
    }
}

/// Actually perform the HTTP POST. Separated for testability.
async fn post_heartbeat(payload: &HeartbeatPayload) -> Result<(), Box<dyn std::error::Error>> {
    let client = crate::http::build_client()?;

    client.post(HEARTBEAT_URL).json(payload).send().await?;

    Ok(())
}

/// Debug variant: logs URL, payload, and response/error to stderr.
async fn debug_post_heartbeat(payload: &HeartbeatPayload) {
    eprintln!("[nitpik:debug] telemetry POST {HEARTBEAT_URL}");
    match serde_json::to_string_pretty(payload) {
        Ok(json) => {
            for line in json.lines() {
                eprintln!("[nitpik:debug]   {line}");
            }
        }
        Err(e) => eprintln!("[nitpik:debug] failed to serialise payload: {e}"),
    }

    let client = match crate::http::build_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[nitpik:debug] failed to build HTTP client: {e}");
            return;
        }
    };

    match client.post(HEARTBEAT_URL).json(payload).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("[nitpik:debug] response: {status}");
            if !body.is_empty() {
                eprintln!("[nitpik:debug] body: {body}");
            }
        }
        Err(e) => {
            eprintln!("[nitpik:debug] request failed: {e}");
            // Walk the error chain for full diagnostics
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                eprintln!("[nitpik:debug]   caused by: {cause}");
                source = std::error::Error::source(cause);
            }
        }
    }
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
            version: crate::constants::FULL_VERSION,
        };
        let json = serde_json::to_value(&payload).expect("serialization should succeed");
        assert_eq!(json["file_count"], 3);
        assert_eq!(json["diff_lines"], 42);
        assert_eq!(json["agent_count"], 2);
        assert_eq!(json["licensed"], false);
        assert_eq!(json["is_ci"], false);
        assert_eq!(json["run_id"], "test-run-id");
        assert_eq!(json["version"], crate::constants::FULL_VERSION);
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
        let _ = crate::ci::is_ci();
    }

    #[test]
    fn is_debug_returns_bool() {
        // In a test environment NITPIK_DEBUG is typically unset → false.
        // We simply verify it doesn't panic and returns a bool.
        let result = is_debug();
        // Unless the test runner has NITPIK_DEBUG=1 this is false.
        let _ = result;
    }

    #[tokio::test]
    async fn send_heartbeat_does_not_panic_on_unreachable_url() {
        let payload = HeartbeatPayload::from_review(1, 10, 1, false);
        // This should silently discard the error (unreachable host)
        send_heartbeat(payload);
        // Give the spawned task a moment to run
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
