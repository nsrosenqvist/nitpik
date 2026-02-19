//! Shared types used across all modules.
//!
//! This module defines the core data structures for findings, diffs,
//! agent definitions, and review context. Other modules import from
//! here rather than reaching into each other's internals.

pub mod agent;
pub mod context;
pub mod diff;
pub mod finding;

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use agent::AgentDefinition;
pub use context::{BaselineContext, ReviewContext};
pub use diff::FileDiff;
pub use finding::Severity;

/// Default agent profile name used when no profile is specified.
pub const DEFAULT_PROFILE: &str = "backend";

/// The resolved input mode for the review.
#[derive(Debug, Clone)]
pub enum InputMode {
    /// Read a pre-computed unified diff from a file.
    DiffFile(PathBuf),
    /// Read a unified diff from stdin.
    Stdin,
    /// Diff against a git branch or commit.
    GitBase(String),
    /// Directly scan a file or directory.
    DirectPath(PathBuf),
}

/// Supported LLM provider backends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderName {
    #[default]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAI,
    Cohere,
    Gemini,
    Perplexity,
    #[serde(rename = "deepseek")]
    DeepSeek,
    #[serde(rename = "xai")]
    XAI,
    Groq,
    /// Any OpenAI-compatible API (e.g. Ollama, Together, local servers).
    #[serde(rename = "openai-compatible")]
    OpenAICompatible,
}

impl fmt::Display for ProviderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderName::Anthropic => write!(f, "anthropic"),
            ProviderName::OpenAI => write!(f, "openai"),
            ProviderName::Cohere => write!(f, "cohere"),
            ProviderName::Gemini => write!(f, "gemini"),
            ProviderName::Perplexity => write!(f, "perplexity"),
            ProviderName::DeepSeek => write!(f, "deepseek"),
            ProviderName::XAI => write!(f, "xai"),
            ProviderName::Groq => write!(f, "groq"),
            ProviderName::OpenAICompatible => write!(f, "openai-compatible"),
        }
    }
}

impl std::str::FromStr for ProviderName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(ProviderName::Anthropic),
            "openai" => Ok(ProviderName::OpenAI),
            "cohere" => Ok(ProviderName::Cohere),
            "gemini" => Ok(ProviderName::Gemini),
            "perplexity" => Ok(ProviderName::Perplexity),
            "deepseek" => Ok(ProviderName::DeepSeek),
            "xai" => Ok(ProviderName::XAI),
            "groq" => Ok(ProviderName::Groq),
            "openai-compatible" => Ok(ProviderName::OpenAICompatible),
            other => Err(format!(
                "unsupported provider: '{other}'. Supported: anthropic, openai, cohere, \
                 gemini, perplexity, deepseek, xai, groq, openai-compatible"
            )),
        }
    }
}

impl ProviderName {
    /// Returns the provider-specific environment variable name for the API key.
    ///
    /// These match the env var names used by rig-core's `from_env()` implementations.
    pub fn api_key_env_var(self) -> &'static str {
        match self {
            ProviderName::Anthropic => "ANTHROPIC_API_KEY",
            ProviderName::OpenAI | ProviderName::OpenAICompatible => "OPENAI_API_KEY",
            ProviderName::Cohere => "COHERE_API_KEY",
            ProviderName::Gemini => "GEMINI_API_KEY",
            ProviderName::Perplexity => "PERPLEXITY_API_KEY",
            ProviderName::DeepSeek => "DEEPSEEK_API_KEY",
            ProviderName::XAI => "XAI_API_KEY",
            ProviderName::Groq => "GROQ_API_KEY",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_display() {
        assert_eq!(ProviderName::Anthropic.to_string(), "anthropic");
        assert_eq!(ProviderName::OpenAI.to_string(), "openai");
        assert_eq!(ProviderName::Cohere.to_string(), "cohere");
        assert_eq!(ProviderName::Gemini.to_string(), "gemini");
        assert_eq!(ProviderName::Perplexity.to_string(), "perplexity");
        assert_eq!(ProviderName::DeepSeek.to_string(), "deepseek");
        assert_eq!(ProviderName::XAI.to_string(), "xai");
        assert_eq!(ProviderName::Groq.to_string(), "groq");
        assert_eq!(
            ProviderName::OpenAICompatible.to_string(),
            "openai-compatible"
        );
    }

    #[test]
    fn provider_name_from_str_all_variants() {
        assert_eq!(
            "anthropic".parse::<ProviderName>().unwrap(),
            ProviderName::Anthropic
        );
        assert_eq!(
            "openai".parse::<ProviderName>().unwrap(),
            ProviderName::OpenAI
        );
        assert_eq!(
            "cohere".parse::<ProviderName>().unwrap(),
            ProviderName::Cohere
        );
        assert_eq!(
            "gemini".parse::<ProviderName>().unwrap(),
            ProviderName::Gemini
        );
        assert_eq!(
            "perplexity".parse::<ProviderName>().unwrap(),
            ProviderName::Perplexity
        );
        assert_eq!(
            "deepseek".parse::<ProviderName>().unwrap(),
            ProviderName::DeepSeek
        );
        assert_eq!("xai".parse::<ProviderName>().unwrap(), ProviderName::XAI);
        assert_eq!("groq".parse::<ProviderName>().unwrap(), ProviderName::Groq);
        assert_eq!(
            "openai-compatible".parse::<ProviderName>().unwrap(),
            ProviderName::OpenAICompatible
        );
    }

    #[test]
    fn provider_name_from_str_case_insensitive() {
        assert_eq!(
            "ANTHROPIC".parse::<ProviderName>().unwrap(),
            ProviderName::Anthropic
        );
        assert_eq!(
            "OpenAI".parse::<ProviderName>().unwrap(),
            ProviderName::OpenAI
        );
        assert_eq!(
            "Gemini".parse::<ProviderName>().unwrap(),
            ProviderName::Gemini
        );
    }

    #[test]
    fn provider_name_from_str_invalid() {
        let result = "invalid".parse::<ProviderName>();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unsupported provider"));
        assert!(err.contains("invalid"));
    }

    #[test]
    fn provider_name_api_key_env_var() {
        assert_eq!(
            ProviderName::Anthropic.api_key_env_var(),
            "ANTHROPIC_API_KEY"
        );
        assert_eq!(ProviderName::OpenAI.api_key_env_var(), "OPENAI_API_KEY");
        assert_eq!(ProviderName::Cohere.api_key_env_var(), "COHERE_API_KEY");
        assert_eq!(ProviderName::Gemini.api_key_env_var(), "GEMINI_API_KEY");
        assert_eq!(
            ProviderName::Perplexity.api_key_env_var(),
            "PERPLEXITY_API_KEY"
        );
        assert_eq!(ProviderName::DeepSeek.api_key_env_var(), "DEEPSEEK_API_KEY");
        assert_eq!(ProviderName::XAI.api_key_env_var(), "XAI_API_KEY");
        assert_eq!(ProviderName::Groq.api_key_env_var(), "GROQ_API_KEY");
        assert_eq!(
            ProviderName::OpenAICompatible.api_key_env_var(),
            "OPENAI_API_KEY"
        );
    }

    #[test]
    fn provider_name_default_is_anthropic() {
        assert_eq!(ProviderName::default(), ProviderName::Anthropic);
    }

    #[test]
    fn provider_name_serde_roundtrip() {
        let name = ProviderName::OpenAICompatible;
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"openai-compatible\"");
        let deserialized: ProviderName = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, name);
    }

    #[test]
    fn provider_name_serde_all_variants() {
        let variants = [
            (ProviderName::Anthropic, "\"anthropic\""),
            (ProviderName::OpenAI, "\"openai\""),
            (ProviderName::Cohere, "\"cohere\""),
            (ProviderName::Gemini, "\"gemini\""),
            (ProviderName::DeepSeek, "\"deepseek\""),
            (ProviderName::XAI, "\"xai\""),
            (ProviderName::Groq, "\"groq\""),
        ];
        for (variant, expected_json) in &variants {
            let json = serde_json::to_string(variant).unwrap();
            assert_eq!(&json, expected_json, "serialize failed for {variant:?}");
            let back: ProviderName = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, variant, "deserialize failed for {expected_json}");
        }
    }
}
