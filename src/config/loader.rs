//! Config struct and loading logic.
//!
//! Priority (highest to lowest):
//! 1. CLI flags
//! 2. Environment variables
//! 3. `.nitpik.toml` in repo root
//! 4. `~/.config/nitpik/config.toml` (global defaults)
//! 5. Built-in defaults

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::models::finding::Severity;
use crate::models::{ProviderName, DEFAULT_PROFILE};

/// Errors during config loading.
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse config file {path}: {source}")]
    ParseFile {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub review: ReviewConfig,
    pub provider: ProviderConfig,
    pub secrets: SecretsConfig,
    pub license: LicenseConfig,
    pub telemetry: TelemetryConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            review: ReviewConfig::default(),
            provider: ProviderConfig::default(),
            secrets: SecretsConfig::default(),
            license: LicenseConfig::default(),
            telemetry: TelemetryConfig::default(),
        }
    }
}

/// Review-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    pub default_profiles: Vec<String>,
    pub fail_on: Option<Severity>,
    pub agentic: AgenticConfig,
    pub context: ContextConfig,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            default_profiles: vec![DEFAULT_PROFILE.to_string()],
            fail_on: None,
            agentic: AgenticConfig::default(),
            context: ContextConfig::default(),
        }
    }
}

/// Agentic mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgenticConfig {
    pub enabled: bool,
    pub max_turns: usize,
    pub max_tool_calls: usize,
}

impl Default for AgenticConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_turns: 10,
            max_tool_calls: 10,
        }
    }
}

/// Context assembly configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub max_file_lines: usize,
    pub surrounding_lines: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_file_lines: 1000,
            surrounding_lines: 100,
        }
    }
}

/// LLM provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub name: ProviderName,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: ProviderName::Anthropic,
            model: "claude-sonnet-4-20250514".to_string(),
            base_url: None,
            api_key: None,
        }
    }
}

/// Secret scanning configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsConfig {
    pub enabled: bool,
    pub additional_rules: Option<String>,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            additional_rules: None,
        }
    }
}

/// License key configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LicenseConfig {
    /// License key string (base64-encoded signed payload).
    pub key: Option<String>,
}

impl Default for LicenseConfig {
    fn default() -> Self {
        Self { key: None }
    }
}

/// Telemetry configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Whether anonymous usage telemetry is enabled.
    pub enabled: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Config {
    /// Load configuration with proper layering.
    ///
    /// Reads from global config, repo-local config, then applies
    /// environment variable overrides.
    pub fn load(repo_root: Option<&Path>) -> Result<Self, ConfigError> {
        let mut config = Config::default();

        // Layer 4: global config
        if let Some(global_path) = Self::global_config_path() {
            if global_path.exists() {
                let global = Self::load_file(&global_path)?;
                config.merge(global);
            }
        }

        // Layer 3: repo-local config
        if let Some(root) = repo_root {
            let local_path = root.join(crate::constants::CONFIG_FILENAME);
            if local_path.exists() {
                let local = Self::load_file(&local_path)?;
                config.merge(local);
            }
        }

        // Layer 2: environment variables
        config.apply_env_vars();

        Ok(config)
    }

    /// Load a config from a specific file.
    fn load_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| ConfigError::ReadFile {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str(&content).map_err(|e| ConfigError::ParseFile {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Get the global config file path.
    fn global_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join(crate::constants::CONFIG_DIR).join("config.toml"))
    }

    /// Merge another config into this one (other takes precedence for non-default values).
    ///
    /// Uses a partial-config pattern: we serialize `other` to TOML value,
    /// then only override fields that were explicitly set (non-default).
    fn merge(&mut self, other: Config) {
        // Review settings
        let default_review = ReviewConfig::default();
        if other.review.default_profiles != default_review.default_profiles {
            self.review.default_profiles = other.review.default_profiles;
        }
        if other.review.fail_on.is_some() {
            self.review.fail_on = other.review.fail_on;
        }
        if other.review.agentic.enabled {
            self.review.agentic.enabled = true;
        }
        if other.review.agentic.max_turns != AgenticConfig::default().max_turns {
            self.review.agentic.max_turns = other.review.agentic.max_turns;
        }
        if other.review.agentic.max_tool_calls != AgenticConfig::default().max_tool_calls {
            self.review.agentic.max_tool_calls = other.review.agentic.max_tool_calls;
        }
        if other.review.context.max_file_lines != ContextConfig::default().max_file_lines {
            self.review.context.max_file_lines = other.review.context.max_file_lines;
        }
        if other.review.context.surrounding_lines != ContextConfig::default().surrounding_lines {
            self.review.context.surrounding_lines = other.review.context.surrounding_lines;
        }

        // Provider settings
        let default_provider = ProviderConfig::default();
        if other.provider.name != default_provider.name {
            self.provider.name = other.provider.name;
        }
        if other.provider.model != default_provider.model {
            self.provider.model = other.provider.model;
        }
        if other.provider.base_url.is_some() {
            self.provider.base_url = other.provider.base_url;
        }
        if other.provider.api_key.is_some() {
            self.provider.api_key = other.provider.api_key;
        }

        // Secret settings
        if other.secrets.enabled {
            self.secrets.enabled = true;
        }
        if other.secrets.additional_rules.is_some() {
            self.secrets.additional_rules = other.secrets.additional_rules;
        }

        // License settings
        if other.license.key.is_some() {
            self.license.key = other.license.key;
        }

        // Telemetry settings (disabled overrides enabled)
        if !other.telemetry.enabled {
            self.telemetry.enabled = false;
        }
    }

    /// Apply environment variable overrides.
    fn apply_env_vars(&mut self) {
        if let Ok(val) = std::env::var(crate::constants::ENV_PROVIDER) {
            if let Ok(name) = val.parse::<ProviderName>() {
                self.provider.name = name;
            } else {
                eprintln!("Warning: ignoring invalid {} value: {val}", crate::constants::ENV_PROVIDER);
            }
        }
        if let Ok(val) = std::env::var(crate::constants::ENV_MODEL) {
            self.provider.model = val;
        }
        if let Ok(val) = std::env::var(crate::constants::ENV_BASE_URL) {
            self.provider.base_url = Some(val);
        }

        // Provider-specific API key resolution
        let api_key = std::env::var(crate::constants::ENV_API_KEY)
            .or_else(|_| std::env::var(self.provider.name.api_key_env_var()))
            .ok();
        if api_key.is_some() {
            self.provider.api_key = api_key;
        }

        // License key
        if let Ok(val) = std::env::var(crate::constants::ENV_LICENSE_KEY) {
            self.license.key = Some(val);
        }

        // Telemetry
        if let Ok(val) = std::env::var(crate::constants::ENV_TELEMETRY) {
            match val.to_lowercase().as_str() {
                "false" | "0" | "no" | "off" => self.telemetry.enabled = false,
                "true" | "1" | "yes" | "on" => self.telemetry.enabled = true,
                _ => eprintln!("Warning: ignoring invalid {} value: {val}", crate::constants::ENV_TELEMETRY),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn default_config() {
        let config = Config::default();
        assert_eq!(config.provider.name, ProviderName::Anthropic);
        assert_eq!(config.provider.model, "claude-sonnet-4-20250514");
        assert_eq!(config.review.default_profiles, vec!["backend"]);
        assert_eq!(config.review.agentic.max_turns, 10);
        assert!(!config.secrets.enabled);
    }

    #[test]
    fn parse_toml_config() {
        let toml_str = r#"
[review]
default_profiles = ["security", "backend"]
fail_on = "warning"

[review.agentic]
enabled = true
max_turns = 5

[provider]
name = "openai"
model = "gpt-4o"

[secrets]
enabled = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider.name, ProviderName::OpenAI);
        assert_eq!(config.provider.model, "gpt-4o");
        assert_eq!(
            config.review.default_profiles,
            vec!["security", "backend"]
        );
        assert!(config.review.agentic.enabled);
        assert_eq!(config.review.agentic.max_turns, 5);
        assert!(config.secrets.enabled);
    }

    #[test]
    fn merge_overrides_non_default_values() {
        let mut base = Config::default();
        let mut other = Config::default();

        other.provider.name = ProviderName::OpenAI;
        other.provider.model = "gpt-4o".to_string();
        other.review.fail_on = Some(Severity::Error);
        other.review.agentic.enabled = true;
        other.review.agentic.max_turns = 5;
        other.review.agentic.max_tool_calls = 3;
        other.review.context.max_file_lines = 500;
        other.review.context.surrounding_lines = 50;
        other.provider.base_url = Some("https://custom.api".to_string());
        other.provider.api_key = Some("sk-test".to_string());
        other.secrets.enabled = true;
        other.secrets.additional_rules = Some("rules.toml".to_string());
        other.review.default_profiles = vec!["security".to_string()];

        base.merge(other);

        assert_eq!(base.provider.name, ProviderName::OpenAI);
        assert_eq!(base.provider.model, "gpt-4o");
        assert_eq!(base.review.fail_on, Some(Severity::Error));
        assert!(base.review.agentic.enabled);
        assert_eq!(base.review.agentic.max_turns, 5);
        assert_eq!(base.review.agentic.max_tool_calls, 3);
        assert_eq!(base.review.context.max_file_lines, 500);
        assert_eq!(base.review.context.surrounding_lines, 50);
        assert_eq!(base.provider.base_url, Some("https://custom.api".to_string()));
        assert_eq!(base.provider.api_key, Some("sk-test".to_string()));
        assert!(base.secrets.enabled);
        assert_eq!(base.secrets.additional_rules, Some("rules.toml".to_string()));
        assert_eq!(base.review.default_profiles, vec!["security"]);
    }

    #[test]
    fn merge_keeps_base_when_other_is_default() {
        let mut base = Config::default();
        base.provider.name = ProviderName::OpenAI;
        base.provider.model = "gpt-4o".to_string();
        base.review.fail_on = Some(Severity::Warning);

        let other = Config::default();
        base.merge(other);

        // Base values should be preserved since other has defaults
        assert_eq!(base.provider.name, ProviderName::OpenAI);
        assert_eq!(base.provider.model, "gpt-4o");
        assert_eq!(base.review.fail_on, Some(Severity::Warning));
    }

    #[test]
    fn load_file_reads_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.toml");
        std::fs::write(
            &path,
            r#"
[provider]
name = "openai"
model = "gpt-4o"
"#,
        )
        .unwrap();

        let config = Config::load_file(&path).unwrap();
        assert_eq!(config.provider.name, ProviderName::OpenAI);
        assert_eq!(config.provider.model, "gpt-4o");
    }

    #[test]
    fn load_file_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "not valid {{ toml").unwrap();

        let result = Config::load_file(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse"));
    }

    #[test]
    fn load_file_not_found() {
        let result = Config::load_file(Path::new("/tmp/nitpik_not_exist_config.toml"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("read"));
    }

    #[test]
    fn load_from_repo_root() {
        // Clean env vars that could interfere
        clear_nitpik_env_vars();
        let _guard = EnvGuard;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".nitpik.toml"),
            r#"
[provider]
name = "openai"
model = "gpt-4o"
"#,
        )
        .unwrap();

        let config = Config::load(Some(dir.path())).unwrap();
        assert_eq!(config.provider.name, ProviderName::OpenAI);
        assert_eq!(config.provider.model, "gpt-4o");
    }

    #[test]
    fn load_without_any_config_files() {
        // Clean env vars that could interfere
        clear_nitpik_env_vars();
        let _guard = EnvGuard;

        let dir = tempfile::tempdir().unwrap();
        // No .nitpik.toml, so we should get defaults
        let config = Config::load(Some(dir.path())).unwrap();
        assert_eq!(config.provider.name, ProviderName::Anthropic);
    }

    #[test]
    fn global_config_path_returns_some() {
        // Just verify it doesn't panic and returns a path
        let path = Config::global_config_path();
        // May be None in CI with no home dir, but shouldn't panic
        if let Some(p) = path {
            assert!(p.to_str().unwrap().contains("nitpik"));
        }
    }

    fn clear_nitpik_env_vars() {
        unsafe {
            std::env::remove_var("NITPIK_PROVIDER");
            std::env::remove_var("NITPIK_API_KEY");
            std::env::remove_var("NITPIK_MODEL");
            std::env::remove_var("NITPIK_BASE_URL");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
    }

    /// Guard that cleans up environment variables when dropped.
    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            clear_nitpik_env_vars();
        }
    }

    #[test]
    #[serial]
    #[ignore] // Mutates process env vars — incompatible with E2E tests that need them
    fn apply_env_vars_all_scenarios() {
        // Clean first and use guard for safety on panic
        clear_nitpik_env_vars();
        let _guard = EnvGuard;

        // Run all env var tests sequentially within a single test
        // to avoid race conditions from parallel test execution.

        // Scenario 1: NITPIK_PROVIDER and NITPIK_API_KEY
        {
            unsafe {
                std::env::set_var("NITPIK_PROVIDER", "openai");
                std::env::set_var("NITPIK_API_KEY", "sk-env-test");
                std::env::remove_var("NITPIK_MODEL");
                std::env::remove_var("NITPIK_BASE_URL");
                std::env::remove_var("ANTHROPIC_API_KEY");
                std::env::remove_var("OPENAI_API_KEY");
            }
            let mut config = Config::default();
            config.apply_env_vars();
            assert_eq!(config.provider.name, ProviderName::OpenAI);
            assert_eq!(config.provider.api_key, Some("sk-env-test".to_string()));
        }

        // Scenario 2: NITPIK_MODEL and NITPIK_BASE_URL
        {
            unsafe {
                std::env::remove_var("NITPIK_PROVIDER");
                std::env::remove_var("NITPIK_API_KEY");
                std::env::set_var("NITPIK_MODEL", "gpt-4-turbo");
                std::env::set_var("NITPIK_BASE_URL", "https://custom.api/v1");
                std::env::remove_var("ANTHROPIC_API_KEY");
                std::env::remove_var("OPENAI_API_KEY");
            }
            let mut config = Config::default();
            config.apply_env_vars();
            assert_eq!(config.provider.model, "gpt-4-turbo");
            assert_eq!(
                config.provider.base_url,
                Some("https://custom.api/v1".to_string())
            );
        }

        // Scenario 3: Invalid NITPIK_PROVIDER falls back to default
        {
            unsafe {
                std::env::set_var("NITPIK_PROVIDER", "not-a-provider");
                std::env::remove_var("NITPIK_API_KEY");
                std::env::remove_var("NITPIK_MODEL");
                std::env::remove_var("NITPIK_BASE_URL");
                std::env::remove_var("ANTHROPIC_API_KEY");
                std::env::remove_var("OPENAI_API_KEY");
            }
            let mut config = Config::default();
            config.apply_env_vars();
            assert_eq!(config.provider.name, ProviderName::Anthropic);
        }

        // Scenario 4: Provider-specific API key fallback
        {
            unsafe {
                std::env::remove_var("NITPIK_PROVIDER");
                std::env::remove_var("NITPIK_API_KEY");
                std::env::remove_var("NITPIK_MODEL");
                std::env::remove_var("NITPIK_BASE_URL");
                std::env::remove_var("OPENAI_API_KEY");
                std::env::set_var("ANTHROPIC_API_KEY", "sk-anthropic-test");
            }
            let mut config = Config::default();
            config.apply_env_vars();
            assert_eq!(
                config.provider.api_key,
                Some("sk-anthropic-test".to_string())
            );
        }

        // _guard cleanup happens automatically via Drop
    }
}
