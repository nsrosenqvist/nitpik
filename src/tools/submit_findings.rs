//! Terminal tool that captures structured review findings.
//!
//! In agentic mode the LLM has tool calling, so instead of relying on
//! the model to emit a JSON-encoded findings array as message text we
//! register a single terminal tool — `submit_findings` — whose schema
//! mirrors `Vec<Finding>`. The agent loop watches for a call to this
//! tool and exits as soon as it lands; the captured findings are then
//! returned to the orchestrator without going through any text parser.
//!
//! Benefits:
//! * Eliminates the markdown-fence / prose-prefix JSON parsing path.
//! * Sidesteps the Gemini "function calling + JSON response mime
//!   type" conflict that forced agentic mode to skip `output_schema`.
//! * The LLM cannot accidentally drop or rename fields: rig-core
//!   validates the tool arguments against the declared schema before
//!   the tool is ever invoked.

use std::sync::{Arc, Mutex};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::models::finding::Finding;

/// Reserved tool name. Custom command tools cannot use this name —
/// see `validate_tool_definition` in `src/agents/parser.rs`.
pub const SUBMIT_FINDINGS_TOOL_NAME: &str = "submit_findings";

/// Shared mutable sink the tool writes to. The agent loop reads from
/// the same `Arc` after the loop exits to recover the structured
/// findings the model submitted.
pub type FindingsSink = Arc<Mutex<Option<Vec<Finding>>>>;

/// Arguments for the terminal tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SubmitFindingsArgs {
    /// Complete list of findings the reviewer wants to report.
    /// Pass `[]` if the diff is clean.
    pub findings: Vec<Finding>,
}

/// Errors are unreachable: the call body cannot fail.
#[derive(Debug, thiserror::Error)]
#[error("submit_findings: {0}")]
pub struct SubmitFindingsError(pub String);

/// Terminal tool for structured-output review responses.
pub struct SubmitFindingsTool {
    sink: FindingsSink,
}

impl SubmitFindingsTool {
    /// Create a new tool plus a handle to read out the captured
    /// findings once the agent loop terminates.
    pub fn new() -> (Self, FindingsSink) {
        let sink: FindingsSink = Arc::new(Mutex::new(None));
        (
            Self {
                sink: Arc::clone(&sink),
            },
            sink,
        )
    }
}

impl Tool for SubmitFindingsTool {
    const NAME: &'static str = SUBMIT_FINDINGS_TOOL_NAME;
    type Error = SubmitFindingsError;
    type Args = SubmitFindingsArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        // Use the auto-generated schema for the args struct so the
        // shape stays in sync with `Finding` automatically.
        let schema = schema_for!(SubmitFindingsArgs);
        let parameters = match serde_json::to_value(&schema) {
            Ok(v) => v,
            // Fall back to a hand-written shape if serialization ever
            // fails (it shouldn't — schemars produces JSON-safe
            // values).
            Err(_) => json!({
                "type": "object",
                "properties": {"findings": {"type": "array"}},
                "required": ["findings"],
            }),
        };

        ToolDefinition {
            name: SUBMIT_FINDINGS_TOOL_NAME.to_string(),
            description: "Submit your final review findings as a structured array. \
                          Call this tool exactly once at the end of your review. \
                          Pass an empty array if the diff has no issues. \
                          Do not write findings as prose; only data passed here \
                          is recorded."
                .to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let n = args.findings.len();
        let mut guard = self.sink.lock().expect("findings sink poisoned");
        // Replace any previously captured set: only the last call wins.
        // The agent loop exits immediately after this call lands so a
        // second call is unreachable in practice.
        *guard = Some(args.findings);
        Ok(format!("recorded {n} findings"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

    fn sample_finding() -> Finding {
        Finding {
            file: "src/lib.rs".into(),
            line: 10,
            end_line: None,
            severity: Severity::Warning,
            title: "test".into(),
            message: "test message".into(),
            suggestion: None,
            agent: "test-agent".into(),
        }
    }

    #[tokio::test]
    async fn captures_findings_into_sink() {
        let (tool, sink) = SubmitFindingsTool::new();
        let result = tool
            .call(SubmitFindingsArgs {
                findings: vec![sample_finding()],
            })
            .await
            .unwrap();
        assert_eq!(result, "recorded 1 findings");
        let stored = sink.lock().unwrap().as_ref().cloned().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].file, "src/lib.rs");
    }

    #[tokio::test]
    async fn empty_findings_array_is_valid() {
        let (tool, sink) = SubmitFindingsTool::new();
        let result = tool
            .call(SubmitFindingsArgs { findings: vec![] })
            .await
            .unwrap();
        assert_eq!(result, "recorded 0 findings");
        let stored = sink.lock().unwrap().as_ref().cloned().unwrap();
        assert!(stored.is_empty());
    }

    #[tokio::test]
    async fn definition_advertises_correct_name_and_schema() {
        let (tool, _) = SubmitFindingsTool::new();
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, SUBMIT_FINDINGS_TOOL_NAME);
        // Schema should at least declare an object with a `findings`
        // property.
        let params = def.parameters;
        assert!(
            params.get("properties").is_some()
                || params.get("$defs").is_some()
                || params.get("definitions").is_some(),
            "schema should describe the args struct: {params}"
        );
    }

    #[tokio::test]
    async fn last_call_overwrites_sink() {
        let (tool, sink) = SubmitFindingsTool::new();
        tool.call(SubmitFindingsArgs {
            findings: vec![sample_finding()],
        })
        .await
        .unwrap();
        tool.call(SubmitFindingsArgs { findings: vec![] })
            .await
            .unwrap();
        let stored = sink.lock().unwrap().as_ref().cloned().unwrap();
        assert!(stored.is_empty());
    }

    #[test]
    fn reserved_name_constant_matches_tool_name_const() {
        assert_eq!(
            SubmitFindingsTool::NAME,
            SUBMIT_FINDINGS_TOOL_NAME,
            "trait NAME must equal exported constant"
        );
    }
}
