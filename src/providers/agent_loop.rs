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
    /// Set when the loop terminated because the model invoked the
    /// configured [`terminal_tool`](LoopConfig::terminal_tool).
    pub terminated_via_tool: Option<String>,
    /// True when the loop appended a self-repair correction message
    /// because the model returned text without calling the terminal
    /// tool. At most one repair is attempted per run.
    pub self_repair_attempted: bool,
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
    /// Optional terminal tool name. When the model calls a tool with
    /// this name the loop exits immediately after recording the tool
    /// result, without making another completion call. Used to wire
    /// in the structured-output `submit_findings` tool.
    pub terminal_tool: Option<String>,
    /// When set, and the model returns a text-only reply without
    /// having called the [`terminal_tool`], the loop appends a
    /// synthetic correction message and lets the model retry **once**.
    /// The correction text is the configured value. Has no effect
    /// when [`terminal_tool`] is `None`.
    pub self_repair_message: Option<String>,
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
            terminal_tool: None,
            self_repair_message: None,
        }
    }

    /// Attach provider-specific extra params.
    pub fn with_additional_params(mut self, params: Option<Value>) -> Self {
        self.additional_params = params;
        self
    }

    /// Mark a tool name as terminal: calling it ends the loop.
    pub fn with_terminal_tool(mut self, name: impl Into<String>) -> Self {
        self.terminal_tool = Some(name.into());
        self
    }

    /// Enable a single self-repair attempt with the given correction
    /// message when the model produces text instead of calling the
    /// terminal tool.
    pub fn with_self_repair(mut self, message: impl Into<String>) -> Self {
        self.self_repair_message = Some(message.into());
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
    let mut self_repair_attempted = false;

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
                terminated_via_tool: None,
                self_repair_attempted,
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
            // Self-repair: if a terminal tool is configured and the
            // model emitted text instead of calling it, append a
            // synthetic correction and let the model retry once.
            if !self_repair_attempted
                && config.terminal_tool.is_some()
                && let Some(repair) = config.self_repair_message.clone()
                && turns < config.max_turns
            {
                self_repair_attempted = true;
                history.push(Message::user(repair));
                continue;
            }
            return Ok(LoopOutcome {
                final_text: join_texts(&texts),
                usage,
                history,
                turns,
                terminated_via_tool: None,
                self_repair_attempted,
            });
        }

        // Detect terminal tool invocation before execution so we can
        // surface it deterministically even if the model issues
        // multiple tool calls in the same turn.
        let terminal_hit = config.terminal_tool.as_deref().and_then(|term| {
            tool_calls.iter().find_map(|c| match c {
                AssistantContent::ToolCall(tc) if tc.function.name == term => {
                    Some(term.to_string())
                }
                _ => None,
            })
        });

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

        if let Some(name) = terminal_hit {
            // Terminal tool fired: stop the loop without making
            // another completion call. final_text is the empty string
            // because the assistant's last action was a tool call;
            // callers know the structured output is in the tool's
            // sink.
            return Ok(LoopOutcome {
                final_text: String::new(),
                usage,
                history,
                turns,
                terminated_via_tool: Some(name),
                self_repair_attempted,
            });
        }
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

    /// Mirrors a terminal-tool call shape: trivially succeeds and
    /// has a different name so the loop can detect it.
    struct FinishTool;

    #[derive(Debug, Deserialize)]
    struct FinishArgs {
        text: String,
    }

    impl rig::tool::Tool for FinishTool {
        const NAME: &'static str = "finish";
        type Error = EchoError;
        type Args = FinishArgs;
        type Output = String;

        async fn definition(&self, _prompt: String) -> ToolDefinition {
            ToolDefinition {
                name: Self::NAME.to_string(),
                description: "Signal completion".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"],
                }),
            }
        }

        async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
            Ok(format!("finished: {}", args.text))
        }
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

    #[tokio::test]
    async fn terminal_tool_call_ends_loop_immediately() {
        // Script keeps emitting tool calls forever. If the terminal
        // tool is not honored, run_agent_loop will hit the script
        // exhausted error before max_turns.
        let model = MockModel::new(vec![
            MockTurn::ToolCall {
                id: "1".into(),
                name: "echo".into(),
                args: serde_json::json!({"text": "explore"}),
            },
            MockTurn::ToolCall {
                id: "2".into(),
                name: "finish".into(),
                args: serde_json::json!({"text": "the end"}),
            },
            // If terminal handling is broken, the loop would now ask
            // for another completion and fail with "script exhausted".
        ]);
        let cfg = LoopConfig::new("p", 1024, 10).with_terminal_tool("finish");
        let outcome = run_agent_loop(
            model,
            "go".into(),
            vec![echo_tool(), Arc::new(FinishTool)],
            cfg,
        )
        .await
        .unwrap();
        assert_eq!(outcome.terminated_via_tool.as_deref(), Some("finish"));
        assert_eq!(outcome.turns, 2);
        assert_eq!(outcome.final_text, "");
        // History: user, assistant(echo), user(echo result), assistant(finish), user(finish result)
        assert_eq!(outcome.history.len(), 5);
    }

    #[tokio::test]
    async fn terminal_tool_call_in_first_turn_ends_loop() {
        let model = MockModel::new(vec![MockTurn::ToolCall {
            id: "1".into(),
            name: "finish".into(),
            args: serde_json::json!({"text": "instant"}),
        }]);
        let cfg = LoopConfig::new("p", 1024, 10).with_terminal_tool("finish");
        let outcome = run_agent_loop(model, "go".into(), vec![Arc::new(FinishTool)], cfg)
            .await
            .unwrap();
        assert_eq!(outcome.terminated_via_tool.as_deref(), Some("finish"));
        assert_eq!(outcome.turns, 1);
    }

    #[tokio::test]
    async fn self_repair_kicks_in_when_text_returned_without_terminal_call() {
        // Round 1: prose only. Round 2 (after repair): terminal call.
        let model = MockModel::new(vec![
            MockTurn::Text("here are my findings: ...".into()),
            MockTurn::ToolCall {
                id: "1".into(),
                name: "finish".into(),
                args: serde_json::json!({"text": "ok"}),
            },
        ]);
        let cfg = LoopConfig::new("p", 1024, 5)
            .with_terminal_tool("finish")
            .with_self_repair("Please call finish.");
        let outcome = run_agent_loop(model, "go".into(), vec![Arc::new(FinishTool)], cfg)
            .await
            .unwrap();
        assert!(outcome.self_repair_attempted);
        assert_eq!(outcome.terminated_via_tool.as_deref(), Some("finish"));
        // History: user, assistant(text), user(repair), assistant(finish call), user(finish result)
        assert_eq!(outcome.history.len(), 5);
    }

    #[tokio::test]
    async fn self_repair_only_attempted_once() {
        // Two consecutive prose responses → loop bails out without
        // looping forever, returns the final text + flag set.
        let model = MockModel::new(vec![
            MockTurn::Text("first prose".into()),
            MockTurn::Text("second prose".into()),
        ]);
        let cfg = LoopConfig::new("p", 1024, 5)
            .with_terminal_tool("finish")
            .with_self_repair("call it!");
        let outcome = run_agent_loop(model, "go".into(), vec![Arc::new(FinishTool)], cfg)
            .await
            .unwrap();
        assert!(outcome.self_repair_attempted);
        assert_eq!(outcome.terminated_via_tool, None);
        assert_eq!(outcome.final_text, "second prose");
        // Two completion calls made, repair message appended once.
        assert_eq!(outcome.turns, 2);
    }

    #[tokio::test]
    async fn self_repair_disabled_returns_text_immediately() {
        let model = MockModel::new(vec![MockTurn::Text("just prose".into())]);
        let cfg = LoopConfig::new("p", 1024, 5).with_terminal_tool("finish");
        let outcome = run_agent_loop(model, "go".into(), vec![Arc::new(FinishTool)], cfg)
            .await
            .unwrap();
        assert!(!outcome.self_repair_attempted);
        assert_eq!(outcome.final_text, "just prose");
        assert_eq!(outcome.turns, 1);
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
