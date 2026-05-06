//! Owned agent loop driving an LLM through tool-calling rounds.
//!
//! Calls a [`CompletionModel`] per turn directly instead of relying on
//! rig-core's built-in [`rig::agent::Agent::prompt`]. Owning the loop
//! gives us:
//!
//! * Aggregated [`Usage`] across every round (input + output tokens
//!   and cache statistics) returned alongside the final text.
//! * Full message history for downstream observability and caching.
//! * A stable seam for future enhancements (self-repair, terminal
//!   tool, streaming, parallel tool execution policies, etc.).
//!
//! The loop terminates when the model returns no tool calls, when the
//! turn budget is exhausted, or when a tool returns a budget-exhausted
//! error. Tool execution errors are forwarded to the model as the tool
//! result string so it can react and try a different approach.

use std::sync::Arc;

use rig::OneOrMany;
use rig::completion::message::{AssistantContent, Message, ToolResultContent, UserContent};
use rig::completion::{CompletionModel, ToolDefinition, Usage};
use rig::tool::ToolDyn;
use serde_json::Value;

use super::ProviderError;

/// Result of a successful agent loop run.
#[derive(Debug, Clone)]
pub struct LoopOutcome {
    /// Concatenated text from the final assistant turn (one string per
    /// `AssistantContent::Text`, joined with newlines).
    pub final_text: String,
    /// Aggregated token usage across every round.
    pub usage: Usage,
    /// Full conversation history excluding the system preamble.
    /// Order: initial user prompt, then alternating assistant /
    /// tool-result messages per round, ending with the final
    /// assistant message that contained no tool calls.
    pub history: Vec<Message>,
    /// Number of completion calls made (one per turn).
    pub turns: usize,
}

/// Static configuration for a loop run.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// System preamble; sent every turn.
    pub preamble: String,
    /// Sampling temperature. `None` lets the provider pick its default.
    pub temperature: Option<f64>,
    /// Maximum tokens for each completion response.
    pub max_tokens: u64,
    /// Hard cap on completion calls. Once reached the loop returns
    /// whatever text the last assistant turn produced; if that turn
    /// was a tool call, [`final_text`](LoopOutcome::final_text) is
    /// empty and the caller's parser will surface the issue.
    pub max_turns: usize,
    /// Provider-specific extras forwarded verbatim (e.g. Anthropic
    /// `cache_control` blocks). Sent every turn.
    pub additional_params: Option<Value>,
}

impl LoopConfig {
    /// Build a config with sensible defaults.
    pub fn new(preamble: impl Into<String>, max_tokens: u64, max_turns: usize) -> Self {
        Self {
            preamble: preamble.into(),
            temperature: Some(0.0),
            max_tokens,
            max_turns: max_turns.max(1),
            additional_params: None,
        }
    }

    /// Attach provider-specific extra params.
    pub fn with_additional_params(mut self, params: Option<Value>) -> Self {
        self.additional_params = params;
        self
    }
}

/// Drive a model through a tool-calling loop and return the final text.
///
/// `tools` is a flat registry of dynamically dispatched tools. Each
/// tool is identified by `tool.name()` and invoked with the JSON
/// argument string the model emits. Tool results — successes and
/// errors alike — are appended as `tool_result` messages so the model
/// can recover.
pub async fn run_agent_loop<M>(
    model: M,
    user_prompt: String,
    tools: Vec<Arc<dyn ToolDyn>>,
    config: LoopConfig,
) -> Result<LoopOutcome, ProviderError>
where
    M: CompletionModel + 'static,
{
    let tool_defs = collect_tool_definitions(&tools, &user_prompt).await;

    let mut history: Vec<Message> = vec![Message::user(user_prompt.clone())];
    let mut usage = Usage::new();
    let mut turns: usize = 0;

    loop {
        if turns >= config.max_turns {
            // Out of budget. Surface whatever the last assistant text
            // was (may be empty if the last action was a tool call).
            let final_text = last_assistant_text(&history);
            return Ok(LoopOutcome {
                final_text,
                usage,
                history,
                turns,
            });
        }

        let prompt_msg = history
            .last()
            .cloned()
            .expect("history always has at least the user prompt");
        let prior = &history[..history.len() - 1];

        let mut builder = model
            .completion_request(prompt_msg)
            .preamble(config.preamble.clone())
            .messages(prior.iter().cloned())
            .max_tokens(config.max_tokens)
            .tools(tool_defs.clone());

        if let Some(temp) = config.temperature {
            builder = builder.temperature(temp);
        }
        if let Some(ref params) = config.additional_params {
            builder = builder.additional_params(params.clone());
        }

        let resp = builder.send().await.map_err(|e| {
            ProviderError::ApiError(format!("completion failed on turn {}: {e}", turns + 1))
        })?;

        usage += resp.usage;
        turns += 1;

        let assistant_msg = Message::Assistant {
            id: resp.message_id.clone(),
            content: resp.choice.clone(),
        };
        history.push(assistant_msg);

        let (tool_calls, texts) = partition_choice(&resp.choice);

        if tool_calls.is_empty() {
            return Ok(LoopOutcome {
                final_text: join_texts(&texts),
                usage,
                history,
                turns,
            });
        }

        // Execute every tool call and feed results back.
        let results = execute_tool_calls(&tools, &tool_calls).await;
        let user_contents: Vec<UserContent> = results
            .into_iter()
            .map(|(id, call_id, output)| {
                UserContent::ToolResult(rig::completion::message::ToolResult {
                    id,
                    call_id,
                    content: OneOrMany::one(ToolResultContent::text(output)),
                })
            })
            .collect();

        let user_msg = Message::User {
            content: OneOrMany::many(user_contents)
                .expect("at least one tool call produced a result"),
        };
        history.push(user_msg);
    }
}

async fn collect_tool_definitions(tools: &[Arc<dyn ToolDyn>], prompt: &str) -> Vec<ToolDefinition> {
    let mut defs = Vec::with_capacity(tools.len());
    for tool in tools {
        defs.push(tool.definition(prompt.to_string()).await);
    }
    defs
}

/// Borrow-only snapshot of the assistant's final response.
fn partition_choice(
    choice: &OneOrMany<AssistantContent>,
) -> (Vec<&AssistantContent>, Vec<&AssistantContent>) {
    choice
        .iter()
        .partition(|c| matches!(c, AssistantContent::ToolCall(_)))
}

fn join_texts(texts: &[&AssistantContent]) -> String {
    texts
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn last_assistant_text(history: &[Message]) -> String {
    for msg in history.iter().rev() {
        if let Message::Assistant { content, .. } = msg {
            let texts: Vec<&AssistantContent> = content
                .iter()
                .filter(|c| matches!(c, AssistantContent::Text(_)))
                .collect();
            if !texts.is_empty() {
                return join_texts(&texts);
            }
        }
    }
    String::new()
}

/// Dispatch every tool call sequentially in arrival order.
///
/// We deliberately serialize execution: the tools are largely
/// I/O-bound, the per-task tool budget is shared mutable state, and
/// the memo cache benefits from earlier reads having landed before
/// later ones run. Concurrency can be reintroduced as a follow-up
/// once budget accounting is reentrant.
async fn execute_tool_calls(
    tools: &[Arc<dyn ToolDyn>],
    calls: &[&AssistantContent],
) -> Vec<(String, Option<String>, String)> {
    let mut out = Vec::with_capacity(calls.len());
    for choice in calls {
        let AssistantContent::ToolCall(call) = choice else {
            continue;
        };
        let id = call.id.clone();
        let call_id = call.call_id.clone();
        let name = call.function.name.clone();
        let args = serde_json::to_string(&call.function.arguments)
            .unwrap_or_else(|e| format!("{{\"_serialization_error\":\"{e}\"}}"));

        let output = match find_tool(tools, &name) {
            Some(tool) => match tool.call(args).await {
                Ok(text) => text,
                Err(err) => format!("tool error: {err}"),
            },
            None => format!("error: unknown tool '{name}'"),
        };
        out.push((id, call_id, output));
    }
    out
}

fn find_tool<'a>(tools: &'a [Arc<dyn ToolDyn>], name: &str) -> Option<&'a Arc<dyn ToolDyn>> {
    tools.iter().find(|t| t.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::completion::message::{Reasoning, ToolCall, ToolFunction};
    use rig::completion::{CompletionError, CompletionRequest, CompletionResponse};
    use serde::{Deserialize, Serialize};
    use std::future::Future;
    use std::sync::Mutex;

    /// A canned response a `MockModel` returns when asked.
    #[derive(Clone)]
    enum MockTurn {
        /// Emit a single tool call with the given name and JSON args.
        ToolCall {
            id: String,
            name: String,
            args: Value,
        },
        /// Emit two tool calls in one turn.
        TwoToolCalls { calls: Vec<(String, String, Value)> },
        /// Emit final assistant text.
        Text(String),
    }

    #[derive(Clone)]
    struct MockModel {
        script: Arc<Mutex<Vec<MockTurn>>>,
        seen_requests: Arc<Mutex<Vec<CompletionRequest>>>,
    }

    impl MockModel {
        fn new(script: Vec<MockTurn>) -> Self {
            Self {
                script: Arc::new(Mutex::new(script)),
                seen_requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl CompletionModel for MockModel {
        type Response = ();
        type StreamingResponse = StubStreaming;
        type Client = ();

        fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
            unreachable!("MockModel is built directly in tests")
        }

        fn completion(
            &self,
            request: CompletionRequest,
        ) -> impl Future<Output = Result<CompletionResponse<Self::Response>, CompletionError>> + Send
        {
            let script = self.script.clone();
            let seen = self.seen_requests.clone();
            async move {
                seen.lock().unwrap().push(clone_request(&request));
                let mut s = script.lock().unwrap();
                if s.is_empty() {
                    return Err(CompletionError::ProviderError(
                        "mock script exhausted".to_string(),
                    ));
                }
                let turn = s.remove(0);
                let choice = match turn {
                    MockTurn::Text(t) => OneOrMany::one(AssistantContent::text(t)),
                    MockTurn::ToolCall { id, name, args } => {
                        OneOrMany::one(AssistantContent::ToolCall(ToolCall {
                            id: id.clone(),
                            call_id: Some(id),
                            function: ToolFunction {
                                name,
                                arguments: args,
                            },
                            signature: None,
                            additional_params: None,
                        }))
                    }
                    MockTurn::TwoToolCalls { calls } => {
                        let mut items = Vec::new();
                        for (id, name, args) in calls {
                            items.push(AssistantContent::ToolCall(ToolCall {
                                id: id.clone(),
                                call_id: Some(id),
                                function: ToolFunction {
                                    name,
                                    arguments: args,
                                },
                                signature: None,
                                additional_params: None,
                            }));
                        }
                        OneOrMany::many(items).unwrap()
                    }
                };
                let mut usage = Usage::new();
                usage.input_tokens = 10;
                usage.output_tokens = 5;
                usage.total_tokens = 15;
                Ok(CompletionResponse {
                    choice,
                    usage,
                    raw_response: (),
                    message_id: None,
                })
            }
        }

        async fn stream(
            &self,
            _request: CompletionRequest,
        ) -> Result<
            rig::streaming::StreamingCompletionResponse<Self::StreamingResponse>,
            CompletionError,
        > {
            Err(CompletionError::ProviderError(
                "streaming not supported in mock".to_string(),
            ))
        }
    }

    fn clone_request(req: &CompletionRequest) -> CompletionRequest {
        // CompletionRequest is not Clone; do a manual shallow snapshot
        // good enough for assertions.
        CompletionRequest {
            model: req.model.clone(),
            preamble: req.preamble.clone(),
            chat_history: req.chat_history.clone(),
            documents: req.documents.clone(),
            tools: req.tools.clone(),
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            tool_choice: req.tool_choice.clone(),
            additional_params: req.additional_params.clone(),
            output_schema: req.output_schema.clone(),
        }
    }

    #[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
    struct StubStreaming {}

    impl rig::completion::GetTokenUsage for StubStreaming {
        fn token_usage(&self) -> Option<Usage> {
            None
        }
    }

    /// Trivial tool that echoes its `text` argument back.
    struct EchoTool;

    #[derive(Debug, thiserror::Error)]
    #[error("echo error: {0}")]
    struct EchoError(String);

    #[derive(Debug, Deserialize)]
    struct EchoArgs {
        text: String,
    }

    impl rig::tool::Tool for EchoTool {
        const NAME: &'static str = "echo";
        type Error = EchoError;
        type Args = EchoArgs;
        type Output = String;

        async fn definition(&self, _prompt: String) -> ToolDefinition {
            ToolDefinition {
                name: Self::NAME.to_string(),
                description: "Echo back the input text".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                }),
            }
        }

        async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
            Ok(format!("echoed: {}", args.text))
        }
    }

    fn echo_tool() -> Arc<dyn ToolDyn> {
        Arc::new(EchoTool)
    }

    #[tokio::test]
    async fn returns_text_when_no_tool_calls() {
        let model = MockModel::new(vec![MockTurn::Text("hello world".into())]);
        let outcome = run_agent_loop(
            model.clone(),
            "ping".into(),
            vec![],
            LoopConfig::new("be helpful", 1024, 5),
        )
        .await
        .unwrap();
        assert_eq!(outcome.final_text, "hello world");
        assert_eq!(outcome.turns, 1);
        assert_eq!(outcome.usage.input_tokens, 10);
        assert_eq!(outcome.usage.output_tokens, 5);
        assert_eq!(outcome.history.len(), 2); // user + assistant
        assert_eq!(model.seen_requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn drives_tool_call_then_text() {
        let model = MockModel::new(vec![
            MockTurn::ToolCall {
                id: "call_1".into(),
                name: "echo".into(),
                args: serde_json::json!({"text": "hi"}),
            },
            MockTurn::Text("done".into()),
        ]);
        let outcome = run_agent_loop(
            model.clone(),
            "use echo".into(),
            vec![echo_tool()],
            LoopConfig::new("preamble", 1024, 5),
        )
        .await
        .unwrap();
        assert_eq!(outcome.final_text, "done");
        assert_eq!(outcome.turns, 2);
        // Aggregated usage across two turns.
        assert_eq!(outcome.usage.input_tokens, 20);
        assert_eq!(outcome.usage.output_tokens, 10);
        // History: user, assistant(tool_call), user(tool_result), assistant(text)
        assert_eq!(outcome.history.len(), 4);
        // Verify tool result message was appended properly.
        let Message::User { content } = &outcome.history[2] else {
            panic!("expected user tool result message");
        };
        let UserContent::ToolResult(tr) = content.iter().next().unwrap() else {
            panic!("expected tool result content");
        };
        assert_eq!(tr.id, "call_1");
        let ToolResultContent::Text(t) = tr.content.iter().next().unwrap() else {
            panic!("expected text tool result");
        };
        assert_eq!(t.text, "echoed: hi");
    }

    #[tokio::test]
    async fn handles_two_tool_calls_in_one_turn() {
        let model = MockModel::new(vec![
            MockTurn::TwoToolCalls {
                calls: vec![
                    (
                        "a".into(),
                        "echo".into(),
                        serde_json::json!({"text": "one"}),
                    ),
                    (
                        "b".into(),
                        "echo".into(),
                        serde_json::json!({"text": "two"}),
                    ),
                ],
            },
            MockTurn::Text("ok".into()),
        ]);
        let outcome = run_agent_loop(
            model,
            "two calls".into(),
            vec![echo_tool()],
            LoopConfig::new("p", 1024, 5),
        )
        .await
        .unwrap();
        assert_eq!(outcome.final_text, "ok");
        // History: user, assistant(2 tc), user(2 results), assistant(text)
        let Message::User { content } = &outcome.history[2] else {
            panic!("expected tool result user message");
        };
        let results: Vec<_> = content.iter().collect();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn forwards_tool_error_to_model() {
        let model = MockModel::new(vec![
            MockTurn::ToolCall {
                id: "x".into(),
                name: "no_such_tool".into(),
                args: serde_json::json!({}),
            },
            MockTurn::Text("recovered".into()),
        ]);
        let outcome = run_agent_loop(
            model,
            "missing".into(),
            vec![echo_tool()],
            LoopConfig::new("p", 1024, 5),
        )
        .await
        .unwrap();
        assert_eq!(outcome.final_text, "recovered");
        let Message::User { content } = &outcome.history[2] else {
            panic!();
        };
        let UserContent::ToolResult(tr) = content.iter().next().unwrap() else {
            panic!()
        };
        let ToolResultContent::Text(t) = tr.content.iter().next().unwrap() else {
            panic!()
        };
        assert!(t.text.contains("unknown tool"));
    }

    #[tokio::test]
    async fn respects_max_turns_budget() {
        // Script keeps issuing tool calls forever; cap at 2 turns.
        let model = MockModel::new(vec![
            MockTurn::ToolCall {
                id: "1".into(),
                name: "echo".into(),
                args: serde_json::json!({"text": "a"}),
            },
            MockTurn::ToolCall {
                id: "2".into(),
                name: "echo".into(),
                args: serde_json::json!({"text": "b"}),
            },
            MockTurn::ToolCall {
                id: "3".into(),
                name: "echo".into(),
                args: serde_json::json!({"text": "c"}),
            },
        ]);
        let outcome = run_agent_loop(
            model,
            "infinite".into(),
            vec![echo_tool()],
            LoopConfig::new("p", 1024, 2),
        )
        .await
        .unwrap();
        assert_eq!(outcome.turns, 2);
        // Last assistant message had only a tool call → no text.
        assert_eq!(outcome.final_text, "");
    }

    #[tokio::test]
    async fn passes_tools_to_provider_each_turn() {
        let model = MockModel::new(vec![
            MockTurn::ToolCall {
                id: "1".into(),
                name: "echo".into(),
                args: serde_json::json!({"text": "x"}),
            },
            MockTurn::Text("end".into()),
        ]);
        run_agent_loop(
            model.clone(),
            "p".into(),
            vec![echo_tool()],
            LoopConfig::new("preamble!", 1024, 5),
        )
        .await
        .unwrap();
        let seen = model.seen_requests.lock().unwrap();
        assert_eq!(seen.len(), 2);
        for req in seen.iter() {
            assert_eq!(req.tools.len(), 1);
            assert_eq!(req.tools[0].name, "echo");
            assert_eq!(req.max_tokens, Some(1024));
        }
    }

    #[tokio::test]
    async fn forwards_additional_params() {
        let model = MockModel::new(vec![MockTurn::Text("done".into())]);
        let extra = serde_json::json!({"cache_control": "ephemeral"});
        let cfg = LoopConfig::new("p", 1024, 5).with_additional_params(Some(extra.clone()));
        run_agent_loop(model.clone(), "p".into(), vec![], cfg)
            .await
            .unwrap();
        let seen = model.seen_requests.lock().unwrap();
        assert_eq!(seen[0].additional_params, Some(extra));
    }

    /// Ensure compilation: AssistantContent::Reasoning variant exists,
    /// guard against silent breakage if rig adds variants we should
    /// also surface as "no tool calls" exits.
    #[test]
    fn reasoning_variant_is_not_a_tool_call() {
        let r = AssistantContent::Reasoning(Reasoning::new("x"));
        assert!(!matches!(r, AssistantContent::ToolCall(_)));
    }
}
