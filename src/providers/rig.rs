//! rig-core integration for LLM-backed code review.
//!
//! Uses rig-core's provider clients and Agent abstraction for multi-provider
//! support. Currently supports: Anthropic, OpenAI, Cohere, Gemini, Perplexity,
//! DeepSeek, xAI, Groq, and any OpenAI-compatible API.
//!
//! In agentic mode (`--agent`), tools are registered with the agent for
//! multi-turn codebase exploration via rig-core's native tool calling.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers;

use crate::config::ProviderConfig;
use crate::models::agent::CustomToolDefinition;
use crate::models::finding::Finding;
use crate::models::{AgentDefinition, ProviderName};
use crate::tools::{CustomCommandTool, ListDirectoryTool, ReadFileTool, SearchTextTool};

use super::{ProviderError, ReviewProvider};

/// Maximum tokens per LLM completion response.
///
/// Set high enough to accommodate thinking models (e.g. Gemini 2.5 Pro)
/// that consume part of the budget for internal reasoning tokens.
const MAX_TOKENS: u64 = 65536;

/// Maximum length of LLM response text to include in parse error messages.
const PARSE_ERROR_PREVIEW_LEN: usize = 2000;

/// Maximum number of retry attempts for transient API errors.
pub const MAX_RETRIES: u32 = 5;

/// Initial backoff delay between retries.
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(10);

/// Maximum backoff delay between retries.
pub const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Build a simple (non-agentic) agent from a rig-core client and prompt it.
///
/// Always sets `max_tokens` — all rig-core providers support it and without
/// it some (e.g. Gemini) default to a low limit that truncates responses.
macro_rules! prompt_simple {
    ($client:expr, $model:expr, $system:expr, $user:expr, $label:expr) => {{
        let agent = $client
            .agent($model)
            .preamble($system)
            .temperature(0.0)
            .max_tokens(MAX_TOKENS)
            .output_schema::<Vec<Finding>>()
            .build();
        agent
            .prompt($user)
            .await
            .map_err(|e| ProviderError::ApiError(format!("{} API error: {e}", $label)))
    }};
}

/// Build an agentic agent with tools from a rig-core client and prompt it.
///
/// Unlike `prompt_simple!`, this intentionally omits `max_tokens` so the
/// model has the full output budget for tool calls and reasoning.
/// Registers the three built-in tools plus any custom command tools
/// defined in the agent profile.
macro_rules! prompt_agentic {
    ($client:expr, $model:expr, $system:expr, $user:expr, $label:expr, $repo:expr, $max_turns:expr, $custom_tools:expr) => {{
        // NOTE: `output_schema` is intentionally omitted here. Setting it
        // alongside tools causes models (especially Gemini) to skip tool
        // calls and immediately return schema-conforming JSON (often `[]`).
        // We rely on the system prompt for JSON format instructions and
        // `parse_findings_response` for parsing.
        //
        // NOTE: `max_tokens` is intentionally omitted. Agentic mode needs
        // the full output budget for tool calls + reasoning tokens. An
        // artificial cap here was causing truncated responses. The model
        // will stop generating when it's done regardless.
        let mut builder = $client
            .agent($model)
            .preamble($system)
            .temperature(0.0)
            .tool(ReadFileTool::new($repo.clone()))
            .tool(SearchTextTool::new($repo.clone()))
            .tool(ListDirectoryTool::new($repo.clone()));

        // Register custom command tools from the agent profile
        for custom_tool in $custom_tools {
            builder = builder.tool(custom_tool);
        }

        let agent = builder.default_max_turns($max_turns).build();
        agent
            .prompt($user)
            .await
            .map_err(|e| ProviderError::ApiError(format!("{} agentic error: {e}", $label)))
    }};
}

/// Create a rig-core client using the `Client::new(api_key)` convention.
macro_rules! new_client {
    ($provider_mod:path, $api_key:expr, $label:expr) => {{
        <$provider_mod>::new($api_key).map_err(|e| {
            ProviderError::ApiError(format!("failed to create {} client: {e}", $label))
        })
    }};
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
        if config.api_key.is_none() {
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

    /// Require `base_url` for OpenAI-compatible providers.
    fn require_base_url(&self) -> Result<&str, ProviderError> {
        self.config.base_url.as_deref().ok_or_else(|| {
            ProviderError::NotConfigured(
                "openai-compatible provider requires base_url to be set".to_string(),
            )
        })
    }

    /// Get the API key or return an error.
    fn api_key(&self) -> Result<&str, ProviderError> {
        self.config
            .api_key
            .as_deref()
            .ok_or_else(|| ProviderError::NotConfigured("missing API key".to_string()))
    }

    /// Make a completion call through rig-core and return the raw response text.
    async fn call_rig(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, ProviderError> {
        let api_key = self.api_key()?;

        match self.config.name {
            ProviderName::Anthropic => {
                let client: providers::anthropic::Client = providers::anthropic::Client::builder()
                    .api_key(api_key)
                    .build()
                    .map_err(|e| {
                        ProviderError::ApiError(format!("failed to create Anthropic client: {e}"))
                    })?;
                prompt_simple!(client, model, system_prompt, user_prompt, "Anthropic")
            }
            ProviderName::OpenAI => {
                let client = self.build_openai_client(api_key)?;
                prompt_simple!(client, model, system_prompt, user_prompt, "OpenAI")
            }
            ProviderName::Cohere => {
                let client = new_client!(providers::cohere::Client, api_key, "Cohere")?;
                prompt_simple!(client, model, system_prompt, user_prompt, "Cohere")
            }
            ProviderName::Gemini => {
                let client = new_client!(providers::gemini::Client, api_key, "Gemini")?;
                prompt_simple!(client, model, system_prompt, user_prompt, "Gemini")
            }
            ProviderName::Perplexity => {
                let client = new_client!(providers::perplexity::Client, api_key, "Perplexity")?;
                prompt_simple!(client, model, system_prompt, user_prompt, "Perplexity")
            }
            ProviderName::DeepSeek => {
                let client = new_client!(providers::deepseek::Client, api_key, "DeepSeek")?;
                prompt_simple!(client, model, system_prompt, user_prompt, "DeepSeek")
            }
            ProviderName::XAI => {
                let client = new_client!(providers::xai::Client, api_key, "xAI")?;
                prompt_simple!(client, model, system_prompt, user_prompt, "xAI")
            }
            ProviderName::Groq => {
                let client = new_client!(providers::groq::Client, api_key, "Groq")?;
                prompt_simple!(client, model, system_prompt, user_prompt, "Groq")
            }
            ProviderName::OpenAICompatible => {
                let base_url = self.require_base_url()?;
                let client: providers::openai::CompletionsClient =
                    providers::openai::CompletionsClient::builder()
                        .api_key(api_key)
                        .base_url(base_url)
                        .build()
                        .map_err(|e| {
                            ProviderError::ApiError(format!(
                                "failed to create OpenAI-compatible client: {e}"
                            ))
                        })?;
                prompt_simple!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "OpenAI-compatible"
                )
            }
        }
    }

    /// Make an agentic call through rig-core with tools registered.
    ///
    /// Uses rig-core's native tool calling support. The agent will
    /// autonomously call tools to explore the codebase. All rig-core
    /// providers support tool calling through the `CompletionModel` trait,
    /// so tools are registered uniformly regardless of provider.
    ///
    /// `max_turns` controls the maximum number of agentic loop iterations
    /// via rig-core's `default_max_turns` on the agent builder.
    ///
    /// `custom_tools` are user-defined command-line tools from the agent profile.
    async fn call_rig_agentic(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
        max_turns: usize,
        custom_tools: Vec<CustomCommandTool>,
    ) -> Result<String, ProviderError> {
        let api_key = self.api_key()?;

        match self.config.name {
            ProviderName::Anthropic => {
                let client: providers::anthropic::Client = providers::anthropic::Client::builder()
                    .api_key(api_key)
                    .build()
                    .map_err(|e| {
                        ProviderError::ApiError(format!("failed to create Anthropic client: {e}"))
                    })?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "Anthropic",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
            }
            ProviderName::OpenAI | ProviderName::OpenAICompatible => {
                let client = self.build_openai_client(api_key)?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "OpenAI",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
            }
            ProviderName::Cohere => {
                let client = new_client!(providers::cohere::Client, api_key, "Cohere")?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "Cohere",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
            }
            ProviderName::Gemini => {
                let client = new_client!(providers::gemini::Client, api_key, "Gemini")?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "Gemini",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
            }
            ProviderName::Perplexity => {
                let client = new_client!(providers::perplexity::Client, api_key, "Perplexity")?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "Perplexity",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
            }
            ProviderName::DeepSeek => {
                let client = new_client!(providers::deepseek::Client, api_key, "DeepSeek")?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "DeepSeek",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
            }
            ProviderName::XAI => {
                let client = new_client!(providers::xai::Client, api_key, "xAI")?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "xAI",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
            }
            ProviderName::Groq => {
                let client = new_client!(providers::groq::Client, api_key, "Groq")?;
                prompt_agentic!(
                    client,
                    model,
                    system_prompt,
                    user_prompt,
                    "Groq",
                    self.repo_root,
                    max_turns,
                    custom_tools
                )
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

            self.call_rig_agentic(
                model,
                &agentic_system_prompt,
                prompt,
                max_turns,
                custom_tools,
            )
            .await
        } else {
            self.call_rig(model, &agent.system_prompt, prompt).await
        };

        match result {
            Ok(response) => parse_findings_response(&response),
            Err(e) => Err(e),
        }
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

/// Check whether a provider error is transient and worth retrying.
///
/// Matches HTTP status codes commonly used for rate limiting and
/// temporary unavailability: 429 (Too Many Requests), 503 (Service
/// Unavailable), 529 (Overloaded), and connection/timeout errors.
///
/// Parse errors are never retried — the LLM is likely to produce the
/// same malformed output on a retry (especially truncated responses).
pub fn is_retryable(err: &ProviderError) -> bool {
    match err {
        ProviderError::ParseError(_) => false,
        _ => classify_error(err).is_some(),
    }
}

/// Classifies a provider error into a short, user-friendly message.
///
/// Returns `Some(message)` for transient/retryable errors, `None` otherwise.
pub fn classify_error(err: &ProviderError) -> Option<&'static str> {
    match err {
        ProviderError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("429")
                || msg_lower.contains("rate limit")
                || msg_lower.contains("too many requests")
            {
                Some("Rate limited by API")
            } else if msg_lower.contains("503")
                || msg_lower.contains("service unavailable")
                || msg_lower.contains("high demand")
            {
                Some("High model load")
            } else if msg_lower.contains("529") || msg_lower.contains("overloaded") {
                Some("API overloaded")
            } else if msg_lower.contains("502") {
                Some("API gateway error")
            } else if msg_lower.contains("timeout") || msg_lower.contains("timed out") {
                Some("Request timed out")
            } else if msg_lower.contains("connection") {
                Some("Connection error")
            } else if msg_lower.contains("temporarily") || msg_lower.contains("try again") {
                Some("Temporary API error")
            } else {
                None
            }
        }
        ProviderError::ParseError(_) => Some("Failed to parse LLM response"),
        _ => None,
    }
}

/// Compute the backoff duration for a retry attempt using exponential backoff.
pub fn retry_backoff(attempt: u32) -> Duration {
    let backoff = INITIAL_BACKOFF.saturating_mul(2u32.saturating_pow(attempt));
    backoff.min(MAX_BACKOFF)
}

/// Parse the LLM response text into structured findings.
///
/// With `output_schema` enforcing the JSON schema at the provider level,
/// the response is expected to be valid JSON. We still handle an empty
/// response or a `{"findings": [...]}` wrapper gracefully.
///
/// Some providers may return JSON wrapped in markdown code fences
/// (e.g. ```json ... ```), so we extract the inner content first.
fn parse_findings_response(response: &str) -> Result<Vec<Finding>, ProviderError> {
    let trimmed = response.trim();

    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // Try the raw text first, then try extracting from markdown fences
    let candidates = extract_json_candidates(trimmed);

    for candidate in &candidates {
        // Try parsing as a direct array of findings
        if let Ok(findings) = serde_json::from_str::<Vec<Finding>>(candidate) {
            return Ok(findings);
        }

        // Try parsing as {"findings": [...]}
        if let Ok(wrapper) = serde_json::from_str::<serde_json::Value>(candidate) {
            if let Some(findings_arr) = wrapper.get("findings") {
                if let Ok(findings) = serde_json::from_value::<Vec<Finding>>(findings_arr.clone()) {
                    return Ok(findings);
                }
            }
        }
    }

    Err(ProviderError::ParseError(format!(
        "could not parse LLM response as findings JSON. Response: {}",
        &response[..response.len().min(PARSE_ERROR_PREVIEW_LEN)]
    )))
}

/// Regex for extracting content inside markdown code fences.
///
/// The closing ``` must appear at the start of a line (`\n````) to avoid
/// matching triple-backticks embedded inside JSON string values (e.g.
/// suggestion fields containing ```rust code examples).
static FENCE_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)```(?:json)?\s*\n(.*?)\n```").unwrap());

/// Extract candidate JSON strings from a response.
///
/// Returns the trimmed response itself plus any content inside markdown
/// code fences (```json ... ``` or ``` ... ```).
fn extract_json_candidates(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    // First candidate: the raw text
    candidates.push(text.to_string());

    // Second: bracket extraction — find the first '[' and last ']'.
    // This is the most robust strategy when the response contains
    // nested code fences inside JSON string values.
    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        if start < end {
            let slice = &text[start..=end];
            candidates.push(slice.to_string());
        }
    }

    // Third: extract content from markdown code fences.
    for cap in FENCE_RE.captures_iter(text) {
        if let Some(inner) = cap.get(1) {
            let inner_trimmed = inner.as_str().trim();
            if !inner_trimmed.is_empty() {
                candidates.push(inner_trimmed.to_string());
            }
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_array() {
        let response = r#"[
            {
                "file": "src/main.rs",
                "line": 42,
                "severity": "error",
                "title": "Bug found",
                "message": "This is a bug",
                "suggestion": "Fix it",
                "agent": "backend"
            }
        ]"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/main.rs");
    }

    #[test]
    fn parse_wrapped_json() {
        let response = r#"{
    "findings": [
        {
            "file": "test.rs",
            "line": 1,
            "severity": "warning",
            "title": "Issue",
            "message": "Problem here",
            "agent": "test"
        }
    ]
}"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn parse_empty_response() {
        let findings = parse_findings_response("").unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_whitespace_only() {
        let findings = parse_findings_response("   \n\n  ").unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_unparseable_response() {
        let result = parse_findings_response("This is random text with no JSON.");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("could not parse"));
    }

    #[test]
    fn parse_empty_json_array() {
        let findings = parse_findings_response("[]").unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_bare_json() {
        let response = r#"[
            {
                "file": "lib.rs",
                "line": 10,
                "severity": "info",
                "title": "Style",
                "message": "Consider renaming",
                "agent": "test"
            }
        ]"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
    }

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
    fn retryable_429_rate_limit() {
        let err = ProviderError::ApiError(
            "Gemini API error: HttpError: Invalid status code 429 Too Many Requests".into(),
        );
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_503_unavailable() {
        let err = ProviderError::ApiError(
            "Gemini API error: HttpError: Invalid status code 503 Service Unavailable".into(),
        );
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_overloaded_message() {
        let err =
            ProviderError::ApiError("Anthropic API error: overloaded — try again later".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn not_retryable_auth_error() {
        let err = ProviderError::ApiError("Invalid API key: 401 Unauthorized".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn not_retryable_parse_error() {
        let err = ProviderError::ParseError("bad json".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn not_retryable_not_configured() {
        let err = ProviderError::NotConfigured("missing key".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn backoff_is_exponential() {
        let b0 = retry_backoff(0);
        let b1 = retry_backoff(1);
        let b2 = retry_backoff(2);
        assert_eq!(b0, Duration::from_secs(10));
        assert_eq!(b1, Duration::from_secs(20));
        assert_eq!(b2, Duration::from_secs(40));
    }

    #[test]
    fn backoff_capped_at_max() {
        let b10 = retry_backoff(10);
        assert_eq!(b10, MAX_BACKOFF);
    }

    #[test]
    fn agentic_system_prompt_includes_tool_instructions() {
        let base = "You are a backend reviewer.";
        let enhanced = build_agentic_system_prompt(base, &[], None);

        // Preserves the original prompt
        assert!(enhanced.starts_with(base));

        // Adds tool guidance
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

        // Custom tools appear in the numbered guidance list
        assert!(
            enhanced.contains("Use `run_tests`"),
            "numbered list should include run_tests"
        );
        assert!(
            enhanced.contains("Use `lint`"),
            "numbered list should include lint"
        );

        // Custom tools appear in the example calls section
        assert!(
            enhanced.contains("`run_tests` with"),
            "examples should include run_tests"
        );
        assert!(
            enhanced.contains("`lint` with"),
            "examples should include lint"
        );

        // Tool with params shows param name in example
        assert!(
            enhanced.contains("\"filter\""),
            "run_tests example should reference filter param"
        );
    }

    #[test]
    fn parse_markdown_fenced_json() {
        let response = r#"Here are the findings:
```json
[
    {
        "file": "src/lib.rs",
        "line": 5,
        "severity": "warning",
        "title": "Unused import",
        "message": "This import is unused",
        "agent": "backend"
    }
]
```
"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/lib.rs");
    }

    #[test]
    fn parse_fenced_without_json_label() {
        let response = "Some preamble text\n```\n[]\n```\n";
        let findings = parse_findings_response(response).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_json_embedded_in_prose() {
        // LLM returns prose with a JSON array buried in the middle.
        let response = r#"I found one issue:
[{"file":"a.rs","line":1,"severity":"info","title":"T","message":"M","agent":"a"}]
That's all."#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn extract_json_candidates_returns_raw_first() {
        let text = r#"[{"a":1}]"#;
        let candidates = extract_json_candidates(text);
        assert!(!candidates.is_empty());
        assert_eq!(candidates[0], text);
    }

    #[test]
    fn extract_json_candidates_no_brackets() {
        let text = "no json here";
        let candidates = extract_json_candidates(text);
        // Should at least contain the raw text
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], text);
    }

    #[test]
    fn classify_error_502_gateway() {
        let err = ProviderError::ApiError("HTTP 502 Bad Gateway".into());
        assert_eq!(classify_error(&err), Some("API gateway error"));
    }

    #[test]
    fn classify_error_timeout() {
        let err = ProviderError::ApiError("request timed out after 30s".into());
        assert_eq!(classify_error(&err), Some("Request timed out"));
    }

    #[test]
    fn classify_error_connection() {
        let err = ProviderError::ApiError("connection refused".into());
        assert_eq!(classify_error(&err), Some("Connection error"));
    }

    #[test]
    fn classify_error_try_again() {
        let err = ProviderError::ApiError("please try again later".into());
        assert_eq!(classify_error(&err), Some("Temporary API error"));
    }

    #[test]
    fn classify_error_returns_none_for_unknown() {
        let err = ProviderError::ApiError("some unknown error".into());
        assert_eq!(classify_error(&err), None);
    }

    #[test]
    fn classify_error_parse_error() {
        let err = ProviderError::ParseError("could not parse JSON".into());
        assert_eq!(classify_error(&err), Some("Failed to parse LLM response"));
    }

    #[test]
    fn extract_json_candidates_nested_fences() {
        // Simulate an LLM response where suggestion fields contain ```rust fences.
        // The bracket extraction should produce a valid candidate even though
        // the inner fences confuse the fence regex.
        let response = "```json\n[\n  {\n    \"file\": \"db.rs\",\n    \"line\": 10,\n    \"severity\": \"error\",\n    \"title\": \"SQL Injection\",\n    \"message\": \"Vulnerable.\",\n    \"suggestion\": \"Use parameterized queries:\\n```\\nrust\\nquery(?)\\n```\",\n    \"agent\": \"backend\"\n  }\n]\n```";
        let candidates = extract_json_candidates(response);
        // At least one candidate should parse as valid JSON
        let parsed = candidates
            .iter()
            .any(|c| serde_json::from_str::<Vec<Finding>>(c).is_ok());
        assert!(
            parsed,
            "should find a parseable candidate despite nested fences"
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
    fn parse_findings_wrapper_with_empty_array() {
        let response = r#"{"findings": []}"#;
        let findings = parse_findings_response(response).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn backoff_attempt_zero_equals_initial() {
        assert_eq!(retry_backoff(0), INITIAL_BACKOFF);
    }

    #[test]
    fn retryable_rate_limit_message() {
        let err = ProviderError::ApiError("rate limit exceeded".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn agentic_prompt_includes_profile_tool_guidance() {
        let base = "You are a code reviewer.";
        let instructions = "Use search_text to trace data flow before flagging injection risks.";
        let result = build_agentic_system_prompt(base, &[], Some(instructions));

        assert!(result.contains("Profile-Specific Tool Guidance"));
        assert!(result.contains(instructions));
        // Should also contain the base prompt
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
