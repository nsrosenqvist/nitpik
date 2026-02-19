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
    Azure,
    Cohere,
    #[serde(rename = "deepseek")]
    DeepSeek,
    Galadriel,
    Gemini,
    Groq,
    #[serde(rename = "huggingface")]
    HuggingFace,
    Hyperbolic,
    Mira,
    Mistral,
    Moonshot,
    Ollama,
    #[serde(rename = "openai")]
    OpenAI,
    #[serde(rename = "openrouter")]
    OpenRouter,
    Perplexity,
    Together,
    #[serde(rename = "xai")]
    XAI,
    /// Any OpenAI-compatible API (e.g. local servers, corporate proxies).
    #[serde(rename = "openai-compatible")]
    OpenAICompatible,
}

impl fmt::Display for ProviderName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderName::Anthropic => write!(f, "anthropic"),
            ProviderName::Azure => write!(f, "azure"),
            ProviderName::Cohere => write!(f, "cohere"),
            ProviderName::DeepSeek => write!(f, "deepseek"),
            ProviderName::Galadriel => write!(f, "galadriel"),
            ProviderName::Gemini => write!(f, "gemini"),
            ProviderName::Groq => write!(f, "groq"),
            ProviderName::HuggingFace => write!(f, "huggingface"),
            ProviderName::Hyperbolic => write!(f, "hyperbolic"),
            ProviderName::Mira => write!(f, "mira"),
            ProviderName::Mistral => write!(f, "mistral"),
            ProviderName::Moonshot => write!(f, "moonshot"),
            ProviderName::Ollama => write!(f, "ollama"),
            ProviderName::OpenAI => write!(f, "openai"),
            ProviderName::OpenRouter => write!(f, "openrouter"),
            ProviderName::Perplexity => write!(f, "perplexity"),
            ProviderName::Together => write!(f, "together"),
            ProviderName::XAI => write!(f, "xai"),
            ProviderName::OpenAICompatible => write!(f, "openai-compatible"),
        }
    }
}

impl std::str::FromStr for ProviderName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "anthropic" => Ok(ProviderName::Anthropic),
            "azure" => Ok(ProviderName::Azure),
            "cohere" => Ok(ProviderName::Cohere),
            "deepseek" => Ok(ProviderName::DeepSeek),
            "galadriel" => Ok(ProviderName::Galadriel),
            "gemini" => Ok(ProviderName::Gemini),
            "groq" => Ok(ProviderName::Groq),
            "huggingface" => Ok(ProviderName::HuggingFace),
            "hyperbolic" => Ok(ProviderName::Hyperbolic),
            "mira" => Ok(ProviderName::Mira),
            "mistral" => Ok(ProviderName::Mistral),
            "moonshot" => Ok(ProviderName::Moonshot),
            "ollama" => Ok(ProviderName::Ollama),
            "openai" => Ok(ProviderName::OpenAI),
            "openrouter" => Ok(ProviderName::OpenRouter),
            "perplexity" => Ok(ProviderName::Perplexity),
            "together" => Ok(ProviderName::Together),
            "xai" => Ok(ProviderName::XAI),
            "openai-compatible" => Ok(ProviderName::OpenAICompatible),
            other => Err(format!(
                "unsupported provider: '{other}'. Supported: anthropic, azure, cohere, \
                 deepseek, galadriel, gemini, groq, huggingface, hyperbolic, mira, \
                 mistral, moonshot, ollama, openai, openrouter, perplexity, together, \
                 xai, openai-compatible"
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
            ProviderName::Azure => "AZURE_OPENAI_API_KEY",
            ProviderName::Cohere => "COHERE_API_KEY",
            ProviderName::DeepSeek => "DEEPSEEK_API_KEY",
            ProviderName::Galadriel => "GALADRIEL_API_KEY",
            ProviderName::Gemini => "GEMINI_API_KEY",
            ProviderName::Groq => "GROQ_API_KEY",
            ProviderName::HuggingFace => "HUGGINGFACE_API_KEY",
            ProviderName::Hyperbolic => "HYPERBOLIC_API_KEY",
            ProviderName::Mira => "MIRA_API_KEY",
            ProviderName::Mistral => "MISTRAL_API_KEY",
            ProviderName::Moonshot => "MOONSHOT_API_KEY",
            ProviderName::Ollama => "OLLAMA_API_KEY",
            ProviderName::OpenAI | ProviderName::OpenAICompatible => "OPENAI_API_KEY",
            ProviderName::OpenRouter => "OPENROUTER_API_KEY",
            ProviderName::Perplexity => "PERPLEXITY_API_KEY",
            ProviderName::Together => "TOGETHER_API_KEY",
            ProviderName::XAI => "XAI_API_KEY",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_display() {
        assert_eq!(ProviderName::Anthropic.to_string(), "anthropic");
        assert_eq!(ProviderName::Azure.to_string(), "azure");
        assert_eq!(ProviderName::Cohere.to_string(), "cohere");
        assert_eq!(ProviderName::DeepSeek.to_string(), "deepseek");
        assert_eq!(ProviderName::Galadriel.to_string(), "galadriel");
        assert_eq!(ProviderName::Gemini.to_string(), "gemini");
        assert_eq!(ProviderName::Groq.to_string(), "groq");
        assert_eq!(ProviderName::HuggingFace.to_string(), "huggingface");
        assert_eq!(ProviderName::Hyperbolic.to_string(), "hyperbolic");
        assert_eq!(ProviderName::Mira.to_string(), "mira");
        assert_eq!(ProviderName::Mistral.to_string(), "mistral");
        assert_eq!(ProviderName::Moonshot.to_string(), "moonshot");
        assert_eq!(ProviderName::Ollama.to_string(), "ollama");
        assert_eq!(ProviderName::OpenAI.to_string(), "openai");
        assert_eq!(ProviderName::OpenRouter.to_string(), "openrouter");
        assert_eq!(ProviderName::Perplexity.to_string(), "perplexity");
        assert_eq!(ProviderName::Together.to_string(), "together");
        assert_eq!(ProviderName::XAI.to_string(), "xai");
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
            "azure".parse::<ProviderName>().unwrap(),
            ProviderName::Azure
        );
        assert_eq!(
            "cohere".parse::<ProviderName>().unwrap(),
            ProviderName::Cohere
        );
        assert_eq!(
            "deepseek".parse::<ProviderName>().unwrap(),
            ProviderName::DeepSeek
        );
        assert_eq!(
            "galadriel".parse::<ProviderName>().unwrap(),
            ProviderName::Galadriel
        );
        assert_eq!(
            "gemini".parse::<ProviderName>().unwrap(),
            ProviderName::Gemini
        );
        assert_eq!("groq".parse::<ProviderName>().unwrap(), ProviderName::Groq);
        assert_eq!(
            "huggingface".parse::<ProviderName>().unwrap(),
            ProviderName::HuggingFace
        );
        assert_eq!(
            "hyperbolic".parse::<ProviderName>().unwrap(),
            ProviderName::Hyperbolic
        );
        assert_eq!("mira".parse::<ProviderName>().unwrap(), ProviderName::Mira);
        assert_eq!(
            "mistral".parse::<ProviderName>().unwrap(),
            ProviderName::Mistral
        );
        assert_eq!(
            "moonshot".parse::<ProviderName>().unwrap(),
            ProviderName::Moonshot
        );
        assert_eq!(
            "ollama".parse::<ProviderName>().unwrap(),
            ProviderName::Ollama
        );
        assert_eq!(
            "openai".parse::<ProviderName>().unwrap(),
            ProviderName::OpenAI
        );
        assert_eq!(
            "openrouter".parse::<ProviderName>().unwrap(),
            ProviderName::OpenRouter
        );
        assert_eq!(
            "perplexity".parse::<ProviderName>().unwrap(),
            ProviderName::Perplexity
        );
        assert_eq!(
            "together".parse::<ProviderName>().unwrap(),
            ProviderName::Together
        );
        assert_eq!("xai".parse::<ProviderName>().unwrap(), ProviderName::XAI);
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
        assert_eq!(
            ProviderName::Azure.api_key_env_var(),
            "AZURE_OPENAI_API_KEY"
        );
        assert_eq!(ProviderName::Cohere.api_key_env_var(), "COHERE_API_KEY");
        assert_eq!(ProviderName::DeepSeek.api_key_env_var(), "DEEPSEEK_API_KEY");
        assert_eq!(
            ProviderName::Galadriel.api_key_env_var(),
            "GALADRIEL_API_KEY"
        );
        assert_eq!(ProviderName::Gemini.api_key_env_var(), "GEMINI_API_KEY");
        assert_eq!(ProviderName::Groq.api_key_env_var(), "GROQ_API_KEY");
        assert_eq!(
            ProviderName::HuggingFace.api_key_env_var(),
            "HUGGINGFACE_API_KEY"
        );
        assert_eq!(
            ProviderName::Hyperbolic.api_key_env_var(),
            "HYPERBOLIC_API_KEY"
        );
        assert_eq!(ProviderName::Mira.api_key_env_var(), "MIRA_API_KEY");
        assert_eq!(ProviderName::Mistral.api_key_env_var(), "MISTRAL_API_KEY");
        assert_eq!(ProviderName::Moonshot.api_key_env_var(), "MOONSHOT_API_KEY");
        assert_eq!(ProviderName::Ollama.api_key_env_var(), "OLLAMA_API_KEY");
        assert_eq!(ProviderName::OpenAI.api_key_env_var(), "OPENAI_API_KEY");
        assert_eq!(
            ProviderName::OpenRouter.api_key_env_var(),
            "OPENROUTER_API_KEY"
        );
        assert_eq!(
            ProviderName::Perplexity.api_key_env_var(),
            "PERPLEXITY_API_KEY"
        );
        assert_eq!(ProviderName::Together.api_key_env_var(), "TOGETHER_API_KEY");
        assert_eq!(ProviderName::XAI.api_key_env_var(), "XAI_API_KEY");
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
            (ProviderName::Azure, "\"azure\""),
            (ProviderName::Cohere, "\"cohere\""),
            (ProviderName::DeepSeek, "\"deepseek\""),
            (ProviderName::Galadriel, "\"galadriel\""),
            (ProviderName::Gemini, "\"gemini\""),
            (ProviderName::Groq, "\"groq\""),
            (ProviderName::HuggingFace, "\"huggingface\""),
            (ProviderName::Hyperbolic, "\"hyperbolic\""),
            (ProviderName::Mira, "\"mira\""),
            (ProviderName::Mistral, "\"mistral\""),
            (ProviderName::Moonshot, "\"moonshot\""),
            (ProviderName::Ollama, "\"ollama\""),
            (ProviderName::OpenAI, "\"openai\""),
            (ProviderName::OpenRouter, "\"openrouter\""),
            (ProviderName::Perplexity, "\"perplexity\""),
            (ProviderName::Together, "\"together\""),
            (ProviderName::XAI, "\"xai\""),
            (ProviderName::OpenAICompatible, "\"openai-compatible\""),
        ];
        for (variant, expected_json) in &variants {
            let json = serde_json::to_string(variant).unwrap();
            assert_eq!(&json, expected_json, "serialize failed for {variant:?}");
            let back: ProviderName = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, variant, "deserialize failed for {expected_json}");
        }
    }
}
