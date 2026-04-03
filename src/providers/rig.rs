//! rig-core integration for LLM-backed code review.
//!
//! Uses rig-core's provider clients and Agent abstraction for multi-provider
//! support. Currently supports: Anthropic, Azure, Cohere, DeepSeek, Galadriel,
//! Gemini, Groq, HuggingFace, Hyperbolic, Mira, Mistral, Moonshot, Ollama,
//! OpenAI, OpenRouter, Perplexity, Together, xAI, and any OpenAI-compatible API.
//!
//! In agentic mode (`--agent`), tools are registered with the agent for
//! multi-turn codebase exploration via rig-core's native tool calling.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers;

use crate::config::ProviderConfig;
use crate::models::agent::CustomToolDefinition;
use crate::models::finding::Finding;
use crate::models::{AgentDefinition, ProviderName};
use crate::providers::response::parse_findings_response;
use crate::tools::{CustomCommandTool, ListDirectoryTool, ReadFileTool, SearchTextTool};

use super::{ProviderError, ReviewProvider};

/// Maximum tokens per LLM completion response.
///
/// Set high enough to accommodate thinking models (e.g. Gemini 2.5 Pro)
/// that consume part of the budget for internal reasoning tokens.
const MAX_TOKENS: u64 = 65536;

/// Map a client-construction error into a [`ProviderError`].
fn map_client_err<T>(
    result: Result<T, impl std::fmt::Display>,
    label: &str,
) -> Result<T, ProviderError> {
    result.map_err(|e| ProviderError::ApiError(format!("failed to create {label} client: {e}")))
}

/// Dispatch a review call through a rig-core client.
///
/// In agentic mode, built-in + custom tools are registered with the agent
/// and the output-schema / max-tokens hints are omitted so the model has
/// full output budget for tool calls. In non-agentic mode, structured
/// output and max-tokens are configured directly.
#[allow(clippy::too_many_arguments)] // Parameters map 1:1 to rig-core API requirements.
async fn dispatch_review<C: CompletionClient>(
    client: &C,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    label: &str,
    agentic: bool,
    repo_root: &Path,
    max_turns: usize,
    custom_tools: Vec<CustomCommandTool>,
) -> Result<String, ProviderError> {
    if agentic {
        let mut builder = client
            .agent(model)
            .preamble(system_prompt)
            .temperature(0.0)
            .tool(ReadFileTool::new(repo_root.to_path_buf()))
            .tool(SearchTextTool::new(repo_root.to_path_buf()))
            .tool(ListDirectoryTool::new(repo_root.to_path_buf()));

        for custom_tool in custom_tools {
            builder = builder.tool(custom_tool);
        }

        let agent = builder.default_max_turns(max_turns).build();
        agent
            .prompt(user_prompt)
            .await
            .map_err(|e| ProviderError::ApiError(format!("{label} agentic error: {e}")))
    } else {
        let agent = client
            .agent(model)
            .preamble(system_prompt)
            .temperature(0.0)
            .max_tokens(MAX_TOKENS)
            .output_schema::<Vec<Finding>>()
            .build();
        agent
            .prompt(user_prompt)
            .await
            .map_err(|e| ProviderError::ApiError(format!("{label} API error: {e}")))
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

    /// Get the API key or return an error.
    fn api_key(&self) -> Result<&str, ProviderError> {
        self.config
            .api_key
            .as_deref()
            .ok_or_else(|| ProviderError::NotConfigured("missing API key".to_string()))
    }

    /// Make a completion call through rig-core, dispatching on provider once.
    ///
    /// In agentic mode, tools are registered with the agent for multi-turn
    /// codebase exploration. `max_turns` and `custom_tools` are only used
    /// when `agentic` is true.
    async fn call(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
        agentic: bool,
        max_turns: usize,
        custom_tools: Vec<CustomCommandTool>,
    ) -> Result<String, ProviderError> {
        // Ollama does not require an API key; all other providers do.
        let api_key = if self.config.name == ProviderName::Ollama {
            self.config.api_key.as_deref().unwrap_or("")
        } else {
            self.api_key()?
        };

        // Helper closure: dispatch to the generic review function after
        // creating a provider-specific client.  Each match arm creates
        // its concrete client and the generic `dispatch_review` handles
        // agent building, tool registration, and prompting.
        let args = (
            model,
            system_prompt,
            user_prompt,
            agentic,
            &self.repo_root,
            max_turns,
            custom_tools,
        );

        match self.config.name {
            ProviderName::Anthropic => {
                let client: providers::anthropic::Client = map_client_err(
                    providers::anthropic::Client::builder()
                        .api_key(api_key)
                        .build(),
                    "Anthropic",
                )?;
                dispatch_review(
                    &client,
                    args.0,
                    args.1,
                    args.2,
                    "Anthropic",
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                )
                .await
            }
            ProviderName::OpenAI => {
                let client = self.build_openai_client(api_key)?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "OpenAI", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Cohere => {
                let client: providers::cohere::Client =
                    map_client_err(providers::cohere::Client::new(api_key), "Cohere")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Cohere", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Gemini => {
                let client: providers::gemini::Client =
                    map_client_err(providers::gemini::Client::new(api_key), "Gemini")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Gemini", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Perplexity => {
                let client: providers::perplexity::Client =
                    map_client_err(providers::perplexity::Client::new(api_key), "Perplexity")?;
                dispatch_review(
                    &client,
                    args.0,
                    args.1,
                    args.2,
                    "Perplexity",
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                )
                .await
            }
            ProviderName::DeepSeek => {
                let client: providers::deepseek::Client =
                    map_client_err(providers::deepseek::Client::new(api_key), "DeepSeek")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "DeepSeek", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::XAI => {
                let client: providers::xai::Client =
                    map_client_err(providers::xai::Client::new(api_key), "xAI")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "xAI", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Groq => {
                let client: providers::groq::Client =
                    map_client_err(providers::groq::Client::new(api_key), "Groq")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Groq", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::HuggingFace => {
                let client: providers::huggingface::Client =
                    map_client_err(providers::huggingface::Client::new(api_key), "HuggingFace")?;
                dispatch_review(
                    &client,
                    args.0,
                    args.1,
                    args.2,
                    "HuggingFace",
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                )
                .await
            }
            ProviderName::Hyperbolic => {
                let client: providers::hyperbolic::Client =
                    map_client_err(providers::hyperbolic::Client::new(api_key), "Hyperbolic")?;
                dispatch_review(
                    &client,
                    args.0,
                    args.1,
                    args.2,
                    "Hyperbolic",
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                )
                .await
            }
            ProviderName::Mira => {
                let client: providers::mira::Client =
                    map_client_err(providers::mira::Client::new(api_key), "Mira")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Mira", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Mistral => {
                let client: providers::mistral::Client =
                    map_client_err(providers::mistral::Client::new(api_key), "Mistral")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Mistral", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Moonshot => {
                let client: providers::moonshot::Client =
                    map_client_err(providers::moonshot::Client::new(api_key), "Moonshot")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Moonshot", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Ollama => {
                let mut builder =
                    providers::ollama::Client::builder().api_key(rig::client::Nothing);
                if let Some(ref base_url) = self.config.base_url {
                    builder = builder.base_url(base_url);
                }
                let client: providers::ollama::Client = map_client_err(builder.build(), "Ollama")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Ollama", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::OpenRouter => {
                let client: providers::openrouter::Client =
                    map_client_err(providers::openrouter::Client::new(api_key), "OpenRouter")?;
                dispatch_review(
                    &client,
                    args.0,
                    args.1,
                    args.2,
                    "OpenRouter",
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                )
                .await
            }
            ProviderName::Together => {
                let client: providers::together::Client =
                    map_client_err(providers::together::Client::new(api_key), "Together")?;
                dispatch_review(
                    &client, args.0, args.1, args.2, "Together", args.3, args.4, args.5, args.6,
                )
                .await
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
                dispatch_review(
                    &client, args.0, args.1, args.2, "Azure", args.3, args.4, args.5, args.6,
                )
                .await
            }
            ProviderName::Galadriel => {
                let client: providers::galadriel::Client =
                    map_client_err(providers::galadriel::Client::new(api_key), "Galadriel")?;
                dispatch_review(
                    &client,
                    args.0,
                    args.1,
                    args.2,
                    "Galadriel",
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                )
                .await
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
                dispatch_review(
                    &client,
                    args.0,
                    args.1,
                    args.2,
                    "OpenAI-compatible",
                    args.3,
                    args.4,
                    args.5,
                    args.6,
                )
                .await
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
        _max_tool_calls: usize,
    ) -> Result<Vec<Finding>, ProviderError> {
        let model = agent.profile.model.as_deref().unwrap_or(&self.config.model);

        let result = if agentic {
            // Build custom command tools from the agent profile
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

            // Enhance the system prompt with tool-usage guidance so the
            // LLM knows it should actively explore before concluding.
            let agentic_system_prompt = build_agentic_system_prompt(
                &agent.system_prompt,
                &agent.profile.tools,
                agent.profile.agentic_instructions.as_deref(),
            );

            self.call(
                model,
                &agentic_system_prompt,
                prompt,
                true,
                max_turns,
                custom_tools,
            )
            .await
        } else {
            self.call(model, &agent.system_prompt, prompt, false, 0, Vec::new())
                .await
        };

        match result {
            Ok(response) => parse_findings_response(&response),
            Err(e) => Err(e),
        }
    }

    async fn complete(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, ProviderError> {
        self.call(
            &self.config.model,
            system_prompt,
            user_prompt,
            false,
            0,
            Vec::new(),
        )
        .await
    }
}

/// Enhance the system prompt for agentic mode.
///
/// Appends instructions that tell the LLM to proactively use tools
/// for codebase exploration before finalising its review findings.
/// The base profile prompt is preserved unchanged; the agentic
/// supplement is appended so it applies regardless of profile.
///
/// Custom tools from the agent profile are included alongside the
/// built-in tools so the LLM knows they are available.
fn build_agentic_system_prompt(
    base_prompt: &str,
    custom_tools: &[CustomToolDefinition],
    agentic_instructions: Option<&str>,
) -> String {
    let mut prompt = format!(
        "{base_prompt}\n\n\
         ## Tool-Assisted Review\n\n\
         You have access to tools for exploring the repository. \
         Use them **proactively** to build a thorough understanding of the code \
         before reporting findings.\n\n\
         When the diff references imports, function calls, types, or modules you \
         have not seen, **use your tools to read the relevant source files** instead \
         of guessing what they contain. Specifically:\n\n\
         1. **Read referenced files** — if the diff imports from or calls into another \
         module, use `read_file` to examine it.\n\
         2. **Search for usages** — use `search_text` to find callers, implementations, \
         or tests related to the changed code.\n\
         3. **Understand the project layout** — use `list_directory` if you are unsure \
         where a file lives or what a module contains.\n\
         4. **Verify before reporting** — do not flag an issue unless you have confirmed \
         it by reading the relevant code. False positives from guessing are worse \
         than a missed finding.\n"
    );

    // Append custom tool guidance
    let mut tool_number = 5;
    for tool in custom_tools {
        prompt.push_str(&format!(
            "         {tool_number}. **Use `{}`** — {}\n",
            tool.name, tool.description
        ));
        tool_number += 1;
    }

    prompt.push_str(
        "\n\
         All tool paths are **relative to the repository root** \
         (e.g., `src/models/finding.rs`, not an absolute path).\n\n\
         ### Example tool calls\n\n\
         - List the repo root: `list_directory` with `{{\"path\": \".\"}}`\n\
         - Read a file: `read_file` with `{{\"path\": \"src/handler.rs\"}}`\n\
         - Search for usages: `search_text` with `{{\"pattern\": \"fn process_updates\"}}`\n",
    );

    // Append custom tool examples
    for tool in custom_tools {
        if let Some(first_param) = tool.parameters.first() {
            prompt.push_str(&format!(
                "         - {}: `{}` with `{{\"{}\":\"...\"}}`\n",
                tool.description, tool.name, first_param.name
            ));
        } else {
            prompt.push_str(&format!(
                "         - {}: `{}` with `{{}}`\n",
                tool.description, tool.name
            ));
        }
    }
    // Profile-specific agentic guidance (from frontmatter `agentic_instructions`)
    if let Some(instructions) = agentic_instructions {
        prompt.push_str(&format!(
            "\n### Profile-Specific Tool Guidance\n\n{instructions}\n"
        ));
    }
    prompt.push_str(
        "\n\
         After exploring, return your findings as a JSON array as described in the \
         instructions.",
    );

    prompt
}

/// Re-export response parsing and retry utilities for backward compatibility.
pub use super::response::{classify_error, is_retryable, retry_backoff};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_provider_missing_api_key() {
        let config = ProviderConfig {
            name: ProviderName::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
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
            model: "claude-sonnet-4-20250514".to_string(),
            base_url: None,
            api_key: Some("sk-test-key".to_string()),
        };
        assert!(RigProvider::new(config, PathBuf::from("/tmp")).is_ok());
    }

    #[test]
    fn new_provider_ollama_no_api_key() {
        let config = ProviderConfig {
            name: ProviderName::Ollama,
            model: "llama3".to_string(),
            base_url: None,
            api_key: None,
        };
        assert!(
            RigProvider::new(config, PathBuf::from("/tmp")).is_ok(),
            "Ollama should not require an API key"
        );
    }

    #[test]
    fn agentic_system_prompt_includes_tool_instructions() {
        let base = "You are a backend reviewer.";
        let enhanced = build_agentic_system_prompt(base, &[], None);

        assert!(enhanced.starts_with(base));
        assert!(enhanced.contains("Tool-Assisted Review"));
        assert!(enhanced.contains("read_file"));
        assert!(enhanced.contains("search_text"));
        assert!(enhanced.contains("list_directory"));
        assert!(enhanced.contains("relative to the repository root"));
        assert!(enhanced.contains("proactively"));
    }

    #[test]
    fn agentic_system_prompt_includes_custom_tools() {
        use crate::models::agent::{CustomToolDefinition, ToolParameter};

        let tools = vec![
            CustomToolDefinition {
                name: "run_tests".to_string(),
                description: "Run the test suite".to_string(),
                command: "cargo test".to_string(),
                parameters: vec![ToolParameter {
                    name: "filter".to_string(),
                    param_type: "string".to_string(),
                    description: "Test name filter".to_string(),
                    required: false,
                }],
            },
            CustomToolDefinition {
                name: "lint".to_string(),
                description: "Run the linter".to_string(),
                command: "cargo clippy".to_string(),
                parameters: vec![],
            },
        ];

        let enhanced = build_agentic_system_prompt("Base prompt.", &tools, None);

        assert!(
            enhanced.contains("Use `run_tests`"),
            "numbered list should include run_tests"
        );
        assert!(
            enhanced.contains("Use `lint`"),
            "numbered list should include lint"
        );
        assert!(
            enhanced.contains("`run_tests` with"),
            "examples should include run_tests"
        );
        assert!(
            enhanced.contains("`lint` with"),
            "examples should include lint"
        );
        assert!(
            enhanced.contains("\"filter\""),
            "run_tests example should reference filter param"
        );
    }

    #[test]
    fn require_base_url_missing() {
        let config = ProviderConfig {
            name: ProviderName::OpenAICompatible,
            model: "custom-model".to_string(),
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
            model: "custom-model".to_string(),
            base_url: Some("https://my-api.example.com".to_string()),
            api_key: Some("key".to_string()),
        };
        let provider = RigProvider::new(config, PathBuf::from("/tmp")).unwrap();
        assert_eq!(
            provider.require_base_url().unwrap(),
            "https://my-api.example.com"
        );
    }

    #[test]
    fn agentic_prompt_includes_profile_tool_guidance() {
        let base = "You are a code reviewer.";
        let instructions = "Use search_text to trace data flow before flagging injection risks.";
        let result = build_agentic_system_prompt(base, &[], Some(instructions));

        assert!(result.contains("Profile-Specific Tool Guidance"));
        assert!(result.contains(instructions));
        assert!(result.contains(base));
    }

    #[test]
    fn agentic_prompt_without_profile_guidance() {
        let base = "You are a code reviewer.";
        let result = build_agentic_system_prompt(base, &[], None);

        assert!(!result.contains("Profile-Specific Tool Guidance"));
        assert!(result.contains(base));
    }
}
