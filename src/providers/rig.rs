//! rig-core integration for LLM-backed code review.
//!
//! Uses rig-core's provider clients and Agent abstraction for multi-provider
//! support. Currently supports: Anthropic, Azure, Cohere, DeepSeek, Galadriel,
//! Gemini, Groq, HuggingFace, Hyperbolic, Mira, Mistral, Moonshot, Ollama,
//! OpenAI, OpenRouter, Perplexity, Together, xAI, and any OpenAI-compatible API.
//!
//! In agentic mode (`--agent`), tools are registered with the agent for
//! multi-turn codebase exploration via rig-core's native tool calling.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, message::AssistantContent};
use rig::providers;
use schemars::{JsonSchema, schema_for};

use crate::config::ProviderConfig;
use crate::models::TokenUsage;
use crate::models::{AgentDefinition, ProviderName};
use crate::orchestrator::prompt::build_agentic_system_prompt;
use crate::providers::response::{parse_findings_response, parse_verdicts_response};
use crate::tools::budget::ToolBudget;
use crate::tools::{
    CustomCommandTool, GlobTool, ListDirectoryTool, ReadFileTool, ReadFilesTool,
    SUBMIT_FINDINGS_TOOL_NAME, SearchTextTool, SubmitFindingsTool,
};

use super::{AgentDiagnostics, ProviderError, ReviewOutcome, ReviewProvider, TriageOutcome};

use crate::constants::MAX_COMPLETION_TOKENS;

/// Map a client-construction error into a [`ProviderError`].
fn map_client_err<T>(
    result: Result<T, impl std::fmt::Display>,
    label: &str,
) -> Result<T, ProviderError> {
    result.map_err(|e| ProviderError::ApiError(format!("failed to create {label} client: {e}")))
}

/// Tool-using agentic configuration for [`dispatch_review`].
struct AgenticConfig {
    repo_root: PathBuf,
    max_turns: usize,
    custom_tools: Vec<CustomCommandTool>,
    /// Hard cap on tool calls scoped to this single review task.
    /// `0` disables enforcement; the budget is still installed so
    /// every tool call goes through `try_consume` for consistency.
    tool_budget: Arc<ToolBudget>,
}

/// Per-call inputs for [`dispatch_review`].
///
/// Bundling these into a struct keeps the per-provider match arms in
/// [`RigProvider::call`] readable. The schema type is supplied as a
/// type parameter at the call site (`call::<Vec<Finding>>(...)`) and
/// constrains the LLM's final response. Sent to providers that support
/// native structured outputs (Anthropic, OpenAI, Gemini, …); ignored
/// by providers that don't, in which case the response parser handles
/// markdown-fenced or prose-prefixed JSON.
struct CallArgs<'a> {
    system_prompt: &'a str,
    user_prompt: &'a str,
    label: &'a str,
    /// Per-call token budget for the final response.
    max_tokens: u64,
    /// When `Some`, the agent registers tools and runs multi-turn.
    agentic: Option<AgenticConfig>,
}

/// Raw output from one provider call. Returned to [`RigProvider`] so
/// it can convert the text into typed findings/verdicts and forward
/// the tokens + diagnostics to the orchestrator.
struct CallResult {
    text: String,
    tokens: TokenUsage,
    diagnostics: AgentDiagnostics,
}

/// Dispatch a single LLM call against a pre-built model handle.
///
/// The handle is built per-provider in [`RigProvider::call`] so each
/// provider can apply its own pre-flight tweaks (e.g. Anthropic's
/// `with_automatic_caching()`) without leaking provider-specific
/// types into this generic body.
///
/// In non-agentic mode the request is built with `output_schema::<T>()`,
/// so providers that support native structured output constrain the
/// response server-side. In agentic mode the schema is **not** set
/// because at least Gemini rejects function calling combined with a
/// JSON response mime type ("Function calling with a response mime
/// type: 'application/json' is unsupported"); other providers may
/// silently ignore it. The agentic prompt itself instructs the LLM to
/// return JSON, and [`parse_with_fallbacks`] handles markdown-fenced
/// or prose-prefixed responses.
/// Strip JSON-Schema validation keywords that strict structured-output
/// schemas reject. schemars emits `minimum` (and `format: "uint32"`) for
/// `u32` fields such as `Finding::line`; Anthropic tool schemas 400 on it
/// ("property 'minimum' is not supported"), and OpenAI strict mode / Gemini
/// reject the same family. Removing them only loosens validation — the data
/// shape (types, required, properties) is unchanged — so it's safe for every
/// provider, not just the strict ones.
fn strip_unsupported_schema_keywords(value: &mut serde_json::Value) {
    const DROP: &[&str] = &[
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "format",
    ];
    match value {
        serde_json::Value::Object(map) => {
            for key in DROP {
                map.remove(*key);
            }
            for child in map.values_mut() {
                strip_unsupported_schema_keywords(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items.iter_mut() {
                strip_unsupported_schema_keywords(child);
            }
        }
        _ => {}
    }
}

/// The structured-output schema for `T`, sanitized for strict providers.
fn review_output_schema<T: JsonSchema>() -> schemars::Schema {
    let mut value = schema_for!(T).to_value();
    strip_unsupported_schema_keywords(&mut value);
    schemars::Schema::try_from(value).expect("sanitized schema is a valid JSON Schema object")
}

async fn dispatch_review<M, T>(model: M, args: CallArgs<'_>) -> Result<CallResult, ProviderError>
where
    M: CompletionModel + 'static,
    T: JsonSchema,
{
    let CallArgs {
        system_prompt,
        user_prompt,
        label,
        max_tokens,
        agentic,
    } = args;

    if let Some(cfg) = agentic {
        // The terminal tool is registered first so the LLM sees it
        // alongside the exploration tools and so call ordering in
        // logs is deterministic.
        let (submit_tool, findings_sink) = SubmitFindingsTool::new();
        let mut tools: Vec<Arc<dyn rig::tool::ToolDyn>> = vec![
            Arc::new(submit_tool),
            Arc::new(ReadFileTool::new(cfg.repo_root.clone())),
            Arc::new(ReadFilesTool::new(cfg.repo_root.clone())),
            Arc::new(SearchTextTool::new(cfg.repo_root.clone())),
            Arc::new(ListDirectoryTool::new(cfg.repo_root.clone())),
            Arc::new(GlobTool::new(cfg.repo_root.clone())),
        ];
        for custom_tool in cfg.custom_tools {
            tools.push(Arc::new(custom_tool));
        }

        let loop_cfg = super::agent_loop::LoopConfig::new(
            system_prompt.to_string(),
            max_tokens,
            cfg.max_turns,
        )
        .with_terminal_tool(SUBMIT_FINDINGS_TOOL_NAME)
        .with_self_repair(
            "Your previous response did not call the `submit_findings` tool. \
             Please call `submit_findings` now with your full list of findings, \
             or with an empty array if there are no issues. Do not write \
             findings as prose.",
        );

        let budget = cfg.tool_budget.clone();
        let prompt_owned = user_prompt.to_string();
        let outcome = crate::tools::budget::scope(budget, async move {
            super::agent_loop::run_agent_loop(model, prompt_owned, tools, loop_cfg).await
        })
        .await
        .map_err(|e| ProviderError::ApiError(format!("{label} agentic error: {e}")))?;

        let tokens: TokenUsage = outcome.usage.into();
        let diagnostics = AgentDiagnostics {
            turns: outcome.turns,
            terminated_via_tool: outcome.terminated_via_tool,
            self_repair_attempted: outcome.self_repair_attempted,
        };

        // If the LLM called submit_findings, return its captured
        // structured payload as JSON so the caller's parser takes the
        // happy path without falling back to text scraping.
        let captured = findings_sink.lock().expect("findings sink poisoned").take();
        if let Some(findings) = captured {
            let text = serde_json::to_string(&findings).map_err(|e| {
                ProviderError::ApiError(format!(
                    "{label} failed to serialize submitted findings: {e}"
                ))
            })?;
            return Ok(CallResult {
                text,
                tokens,
                diagnostics,
            });
        }

        // No terminal tool call. Fall back to whatever text the model
        // produced; the caller's parser can still rescue JSON the
        // model emitted as prose.
        Ok(CallResult {
            text: outcome.final_text,
            tokens,
            diagnostics,
        })
    } else {
        // Non-agentic path: drive `completion_request` directly so we
        // can read `CompletionResponse::usage`.
        let request = model
            .completion_request(user_prompt.to_string())
            .preamble(system_prompt.to_string())
            .temperature(0.0)
            .max_tokens(max_tokens)
            .output_schema(review_output_schema::<T>());

        let response = request
            .send()
            .await
            .map_err(|e| ProviderError::ApiError(format!("{label} API error: {e}")))?;

        let tokens: TokenUsage = response.usage.into();

        // Concatenate every text fragment in the assistant's choice.
        // Tool calls are ignored here — non-agentic mode does not
        // register any tools — but we tolerate them defensively.
        let mut text = String::new();
        for piece in response.choice.iter() {
            if let AssistantContent::Text(t) = piece {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&t.text);
            }
        }
        Ok(CallResult {
            text,
            tokens,
            diagnostics: AgentDiagnostics::default(),
        })
    }
}

/// rig-core based review provider.
///
/// Wraps rig-core's multi-provider client system. The provider name
/// in config selects which rig-core provider to use.
pub struct RigProvider {
    config: ProviderConfig,
    repo_root: PathBuf,
}

impl RigProvider {
    /// Create a new RigProvider with the given configuration.
    pub fn new(config: ProviderConfig, repo_root: PathBuf) -> Result<Self, ProviderError> {
        // Ollama runs locally and does not require an API key.
        if config.api_key.is_none() && config.name != ProviderName::Ollama {
            return Err(ProviderError::NotConfigured(format!(
                "no API key found for provider '{}'. Set {} or the provider-specific env var.",
                config.name,
                crate::constants::ENV_API_KEY
            )));
        }
        Ok(Self { config, repo_root })
    }

    /// Build an OpenAI-style client, optionally with a custom base URL.
    fn build_openai_client(
        &self,
        api_key: &str,
    ) -> Result<providers::openai::CompletionsClient, ProviderError> {
        let mut builder = providers::openai::CompletionsClient::builder().api_key(api_key);
        if let Some(ref base_url) = self.config.base_url {
            builder = builder.base_url(base_url);
        }
        let client: providers::openai::CompletionsClient = builder
            .build()
            .map_err(|e| ProviderError::ApiError(format!("failed to create OpenAI client: {e}")))?;
        Ok(client)
    }

    /// Require `base_url` for providers that need a custom endpoint.
    fn require_base_url(&self) -> Result<&str, ProviderError> {
        self.config.base_url.as_deref().ok_or_else(|| {
            let hint = match self.config.name {
                ProviderName::Azure => {
                    "azure provider requires base_url (your Azure endpoint, e.g. \
                     https://{resource}.openai.azure.com)"
                }
                _ => "openai-compatible provider requires base_url to be set",
            };
            ProviderError::NotConfigured(hint.to_string())
        })
    }

    /// Clamp [`MAX_COMPLETION_TOKENS`] to the provider's per-call cap.
    fn resolved_max_tokens(&self) -> u64 {
        match self.config.name.max_completion_tokens() {
            Some(cap) => MAX_COMPLETION_TOKENS.min(cap),
            None => MAX_COMPLETION_TOKENS,
        }
    }

    /// Get the API key or return an error.
    fn api_key(&self) -> Result<&str, ProviderError> {
        self.config
            .api_key
            .as_deref()
            .ok_or_else(|| ProviderError::NotConfigured("missing API key".to_string()))
    }

    /// Make a completion call through rig-core, dispatching on provider once.
    ///
    /// `T` is the JSON-schema-deriving type that constrains the model's
    /// final response (e.g. [`FindingsResponse`] for review,
    /// [`VerdictsResponse`] for triage — both wrap their inner array in
    /// an object so the schema is OpenAI-spec compliant). Each match arm constructs the
    ///
    /// [`FindingsResponse`]: crate::providers::FindingsResponse
    /// [`VerdictsResponse`]: crate::providers::VerdictsResponse
    /// concrete provider client, builds the model handle (applying any
    /// provider-specific tweaks such as Anthropic prompt caching), and
    /// forwards to [`dispatch_review`].
    ///
    /// **Caching note.** Most providers do prompt caching implicitly
    /// server-side as long as repeat calls share an identical prefix
    /// (OpenAI, Azure, Gemini 2.5, DeepSeek, …) — no opt-in needed and
    /// the savings show up in `usage.cached_input_tokens` automatically.
    /// Anthropic is the exception: caching is opt-in via
    /// [`with_automatic_caching`] on the model handle, applied below.
    ///
    /// [`with_automatic_caching`]: rig::providers::anthropic::completion::CompletionModel::with_automatic_caching
    async fn call<T: JsonSchema>(
        &self,
        model_id: &str,
        args: CallArgs<'_>,
    ) -> Result<CallResult, ProviderError> {
        // Ollama does not require an API key; all other providers do.
        let api_key = if self.config.name == ProviderName::Ollama {
            self.config.api_key.as_deref().unwrap_or("")
        } else {
            self.api_key()?
        };

        match self.config.name {
            ProviderName::Anthropic => {
                let client: providers::anthropic::Client = map_client_err(
                    providers::anthropic::Client::builder()
                        .api_key(api_key)
                        .build(),
                    "Anthropic",
                )?;
                // Anthropic caching is opt-in; everyone else is implicit.
                let model = client.completion_model(model_id).with_automatic_caching();
                dispatch_review::<_, T>(model, args).await
            }
            ProviderName::OpenAI => {
                let client = self.build_openai_client(api_key)?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Cohere => {
                let client: providers::cohere::Client =
                    map_client_err(providers::cohere::Client::new(api_key), "Cohere")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Gemini => {
                let client: providers::gemini::Client =
                    map_client_err(providers::gemini::Client::new(api_key), "Gemini")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Perplexity => {
                let client: providers::perplexity::Client =
                    map_client_err(providers::perplexity::Client::new(api_key), "Perplexity")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::DeepSeek => {
                let client: providers::deepseek::Client =
                    map_client_err(providers::deepseek::Client::new(api_key), "DeepSeek")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::XAI => {
                let client: providers::xai::Client =
                    map_client_err(providers::xai::Client::new(api_key), "xAI")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Groq => {
                let client: providers::groq::Client =
                    map_client_err(providers::groq::Client::new(api_key), "Groq")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::HuggingFace => {
                let client: providers::huggingface::Client =
                    map_client_err(providers::huggingface::Client::new(api_key), "HuggingFace")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Hyperbolic => {
                let client: providers::hyperbolic::Client =
                    map_client_err(providers::hyperbolic::Client::new(api_key), "Hyperbolic")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Mira => {
                let client: providers::mira::Client =
                    map_client_err(providers::mira::Client::new(api_key), "Mira")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Mistral => {
                let client: providers::mistral::Client =
                    map_client_err(providers::mistral::Client::new(api_key), "Mistral")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Moonshot => {
                let client: providers::moonshot::Client =
                    map_client_err(providers::moonshot::Client::new(api_key), "Moonshot")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Ollama => {
                let mut builder =
                    providers::ollama::Client::builder().api_key(rig::client::Nothing);
                if let Some(ref base_url) = self.config.base_url {
                    builder = builder.base_url(base_url);
                }
                let client: providers::ollama::Client = map_client_err(builder.build(), "Ollama")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::OpenRouter => {
                let client: providers::openrouter::Client =
                    map_client_err(providers::openrouter::Client::new(api_key), "OpenRouter")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Together => {
                let client: providers::together::Client =
                    map_client_err(providers::together::Client::new(api_key), "Together")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Azure => {
                let base_url = self.require_base_url()?;
                let client: providers::azure::Client = map_client_err(
                    providers::azure::Client::builder()
                        .api_key(providers::azure::AzureOpenAIAuth::ApiKey(
                            api_key.to_string(),
                        ))
                        .azure_endpoint(base_url.to_string())
                        .build(),
                    "Azure",
                )?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::Galadriel => {
                let client: providers::galadriel::Client =
                    map_client_err(providers::galadriel::Client::new(api_key), "Galadriel")?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::GitHub => {
                let base_url = self
                    .config
                    .base_url
                    .as_deref()
                    .unwrap_or("https://models.github.ai/inference");
                let client: providers::openai::CompletionsClient = map_client_err(
                    providers::openai::CompletionsClient::builder()
                        .api_key(api_key)
                        .base_url(base_url)
                        .build(),
                    "GitHub Models",
                )?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
            ProviderName::OpenAICompatible => {
                let base_url = self.require_base_url()?;
                let client: providers::openai::CompletionsClient = map_client_err(
                    providers::openai::CompletionsClient::builder()
                        .api_key(api_key)
                        .base_url(base_url)
                        .build(),
                    "OpenAI-compatible",
                )?;
                dispatch_review::<_, T>(client.completion_model(model_id), args).await
            }
        }
    }
}

#[async_trait]
impl ReviewProvider for RigProvider {
    async fn review(
        &self,
        agent: &AgentDefinition,
        prompt: &str,
        agentic: bool,
        max_turns: usize,
        max_tool_calls: usize,
    ) -> Result<ReviewOutcome, ProviderError> {
        let model = agent
            .profile
            .model
            .as_deref()
            .unwrap_or_else(|| self.config.resolved_model());

        let agentic_system_prompt;
        let (system_prompt, agentic_cfg) = if agentic {
            let custom_tools: Vec<CustomCommandTool> = agent
                .profile
                .tools
                .iter()
                .map(|def| {
                    CustomCommandTool::new(
                        def,
                        self.repo_root.clone(),
                        agent.profile.environment.clone(),
                    )
                })
                .collect();

            agentic_system_prompt = build_agentic_system_prompt(
                &agent.system_prompt,
                &agent.profile.tools,
                agent.profile.agentic_instructions.as_deref(),
            );

            (
                agentic_system_prompt.as_str(),
                Some(AgenticConfig {
                    repo_root: self.repo_root.clone(),
                    max_turns,
                    custom_tools,
                    tool_budget: ToolBudget::new(max_tool_calls),
                }),
            )
        } else {
            (agent.system_prompt.as_str(), None)
        };

        let result = self
            .call::<crate::providers::FindingsResponse>(
                model,
                CallArgs {
                    system_prompt,
                    user_prompt: prompt,
                    label: "Review",
                    max_tokens: self.resolved_max_tokens(),
                    agentic: agentic_cfg,
                },
            )
            .await?;

        let findings = parse_findings_response(&result.text)?;
        Ok(ReviewOutcome {
            findings,
            tokens: result.tokens,
            diagnostics: result.diagnostics,
        })
    }

    async fn triage(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<TriageOutcome, ProviderError> {
        let result = self
            .call::<crate::providers::VerdictsResponse>(
                self.config.resolved_model(),
                CallArgs {
                    system_prompt,
                    user_prompt,
                    label: "Triage",
                    max_tokens: self.resolved_max_tokens(),
                    agentic: None,
                },
            )
            .await?;

        if result.text.trim().is_empty() {
            return Ok(TriageOutcome {
                verdicts: Vec::new(),
                tokens: result.tokens,
            });
        }

        let verdicts = parse_verdicts_response(&result.text)?;
        Ok(TriageOutcome {
            verdicts,
            tokens: result.tokens,
        })
    }

    async fn summarize(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<crate::providers::SummaryOutcome, ProviderError> {
        let result = self
            .call::<crate::providers::PrSummaryResponse>(
                self.config.resolved_model(),
                CallArgs {
                    system_prompt,
                    user_prompt,
                    label: "Summary",
                    max_tokens: self.resolved_max_tokens(),
                    agentic: None,
                },
            )
            .await?;

        // Structured-output yields `{"summary": "..."}`; if the model
        // emitted prose instead (lenient endpoints), fall back to the raw
        // text rather than failing the whole run.
        let summary = serde_json::from_str::<crate::providers::PrSummaryResponse>(&result.text)
            .map(|r| r.summary)
            .unwrap_or_else(|_| result.text.trim().to_string());

        Ok(crate::providers::SummaryOutcome {
            summary,
            tokens: result.tokens,
        })
    }
}

/// Re-export response parsing and retry utilities for backward compatibility.
pub use super::response::{classify_error, is_retryable, retry_backoff};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_unsupported_schema_keywords_recurses() {
        let mut v = serde_json::json!({
            "type": "object",
            "properties": {
                "line": { "type": "integer", "format": "uint32", "minimum": 0.0 },
                "range": { "type": "array", "items": { "type": "integer", "maximum": 10 } }
            }
        });
        strip_unsupported_schema_keywords(&mut v);
        let props = &v["properties"];
        assert!(props["line"].get("minimum").is_none(), "minimum stripped");
        assert!(props["line"].get("format").is_none(), "format stripped");
        assert_eq!(props["line"]["type"], "integer", "type preserved");
        assert!(
            props["range"]["items"].get("maximum").is_none(),
            "nested stripped"
        );
    }

    #[test]
    fn review_output_schema_drops_minimum_for_u32_fields() {
        // Finding::line is u32, which schemars annotates with `minimum` —
        // the keyword Anthropic's tool schema rejects. It must be gone.
        let schema = review_output_schema::<crate::providers::FindingsResponse>();
        let text = serde_json::to_string(&schema.to_value()).unwrap();
        assert!(
            !text.contains("\"minimum\""),
            "schema must not carry `minimum`: {text}"
        );
    }

    #[test]
    fn new_provider_missing_api_key() {
        let config = ProviderConfig {
            name: ProviderName::Anthropic,
            model: Some("claude-sonnet-4-20250514".to_string()),
            base_url: None,
            api_key: None,
        };
        let result = RigProvider::new(config, PathBuf::from("/tmp"));
        match result {
            Err(e) => assert!(e.to_string().contains("API key"), "got: {e}"),
            Ok(_) => panic!("expected error for missing API key"),
        }
    }

    #[test]
    fn new_provider_with_api_key() {
        let config = ProviderConfig {
            name: ProviderName::Anthropic,
            model: Some("claude-sonnet-4-20250514".to_string()),
            base_url: None,
            api_key: Some("sk-test-key".to_string()),
        };
        assert!(RigProvider::new(config, PathBuf::from("/tmp")).is_ok());
    }

    #[test]
    fn new_provider_ollama_no_api_key() {
        let config = ProviderConfig {
            name: ProviderName::Ollama,
            model: Some("llama3".to_string()),
            base_url: None,
            api_key: None,
        };
        assert!(
            RigProvider::new(config, PathBuf::from("/tmp")).is_ok(),
            "Ollama should not require an API key"
        );
    }

    #[test]
    fn require_base_url_missing() {
        let config = ProviderConfig {
            name: ProviderName::OpenAICompatible,
            model: Some("custom-model".to_string()),
            base_url: None,
            api_key: Some("key".to_string()),
        };
        let provider = RigProvider::new(config, PathBuf::from("/tmp")).unwrap();
        let result = provider.require_base_url();
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("base_url"),
            "should mention base_url"
        );
    }

    #[test]
    fn require_base_url_present() {
        let config = ProviderConfig {
            name: ProviderName::OpenAICompatible,
            model: Some("custom-model".to_string()),
            base_url: Some("https://my-api.example.com".to_string()),
            api_key: Some("key".to_string()),
        };
        let provider = RigProvider::new(config, PathBuf::from("/tmp")).unwrap();
        assert_eq!(
            provider.require_base_url().unwrap(),
            "https://my-api.example.com"
        );
    }
}
