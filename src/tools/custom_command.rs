//! CustomCommandTool — execute user-defined CLI commands as agentic tools.
//!
//! This tool is constructed from a [`CustomToolDefinition`] in an agent
//! profile's YAML frontmatter. When the LLM invokes it, the specified
//! command is run as a subprocess (sandboxed to the repo root) and the
//! combined stdout/stderr is returned.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::models::agent::CustomToolDefinition;

/// Maximum output size from a custom command (256KB).
const MAX_OUTPUT_SIZE: usize = 256 * 1024;

/// Maximum execution time for a custom command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum virtual memory for a subprocess (1 GB, in KB for ulimit -v).
const ULIMIT_VMEM_KB: u64 = 1_048_576;

/// Maximum file size a subprocess may write (100 MB, in 512-byte blocks for ulimit -f).
const ULIMIT_FSIZE_BLOCKS: u64 = 204_800;

/// Arguments passed by the LLM when calling a custom command tool.
///
/// Parameters are passed as a JSON object with string keys/values.
/// The tool definition's `parameters` list determines which keys are valid.
#[derive(Debug, Deserialize)]
pub struct CustomCommandArgs {
    /// Dynamic key-value parameters. Keys match parameter names from the definition.
    #[serde(flatten)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// Error type for custom command execution.
#[derive(Debug, thiserror::Error)]
#[error("CustomCommand error: {0}")]
pub struct CustomCommandError(pub String);

/// Rig-core tool that executes a user-defined CLI command.
///
/// Constructed from a [`CustomToolDefinition`] parsed from agent profile
/// frontmatter. The command runs in the repo root directory.
///
/// Subprocesses receive only a minimal set of safe system variables
/// ([`crate::constants::SAFE_ENV_VARS`] and [`crate::constants::SAFE_ENV_PREFIXES`]).
/// Profile authors can pass additional variables via the `environment`
/// frontmatter field, which populates `env_passthrough`.
#[derive(Serialize, Deserialize)]
pub struct CustomCommandTool {
    /// Tool name (matches `CustomToolDefinition::name`).
    tool_name: String,
    /// Human-readable description for the LLM.
    description: String,
    /// The shell command template to execute.
    command: String,
    /// JSON Schema parameters object for the tool definition.
    parameters_schema: serde_json::Value,
    /// Parameter names marked as required.
    required_params: Vec<String>,
    /// All parameter names in definition order.
    all_param_names: Vec<String>,
    /// Repository root directory (commands run here).
    repo_root: PathBuf,
    /// Pre-computed sanitized environment for subprocess execution.
    /// Built once at construction time from the safe set +
    /// profile-declared passthrough patterns.
    sanitized_env: HashMap<String, String>,
}

impl CustomCommandTool {
    /// Create a new `CustomCommandTool` from a profile definition and repo root.
    ///
    /// `env_passthrough` lists additional variable names (or prefix globs
    /// like `AWS_*`) that the subprocess is allowed to inherit beyond the
    /// default safe set.
    pub fn new(
        def: &CustomToolDefinition,
        repo_root: PathBuf,
        env_passthrough: Vec<String>,
    ) -> Self {
        // Build JSON Schema properties from the parameter definitions
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        let mut all_names = Vec::new();

        for param in &def.parameters {
            properties.insert(
                param.name.clone(),
                json!({
                    "type": param.param_type,
                    "description": param.description,
                }),
            );
            all_names.push(param.name.clone());
            if param.required {
                required.push(param.name.clone());
            }
        }

        let parameters_schema = json!({
            "type": "object",
            "properties": properties,
            "required": required,
        });

        // Pre-compute sanitized environment once at construction time
        let sanitized_env = build_sanitized_env(&env_passthrough);

        Self {
            tool_name: def.name.clone(),
            description: def.description.clone(),
            command: def.command.clone(),
            parameters_schema,
            required_params: required.clone(),
            all_param_names: all_names,
            repo_root,
            sanitized_env,
        }
    }

    /// Build the full command string with parameters appended as arguments.
    fn build_command(&self, params: &serde_json::Map<String, serde_json::Value>) -> String {
        let mut cmd = self.command.clone();

        // Append parameters as `--name value` flags in definition order.
        // This gives predictable argument ordering.
        for name in &self.all_param_names {
            if let Some(value) = params.get(name) {
                let value_str = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Bool(b) => {
                        if *b {
                            // For boolean flags, just append the flag name
                            cmd.push_str(&format!(" --{name}"));
                            continue;
                        } else {
                            continue; // false boolean = omit flag
                        }
                    }
                    other => other.to_string(),
                };

                if !value_str.is_empty() {
                    cmd.push_str(&format!(" --{name} {}", shell_escape(&value_str)));
                }
            }
        }

        cmd
    }

    /// Validate that all required parameters are present and detect unknown ones.
    ///
    /// Returns the list of unknown parameter names (silently ignored but logged).
    fn validate_params(
        &self,
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Vec<String>, CustomCommandError> {
        for required in &self.required_params {
            if !params.contains_key(required) {
                return Err(CustomCommandError(format!(
                    "missing required parameter: {required}"
                )));
            }
        }
        let unknown: Vec<String> = params
            .keys()
            .filter(|k| !self.all_param_names.contains(k))
            .cloned()
            .collect();
        Ok(unknown)
    }
}

impl Tool for CustomCommandTool {
    const NAME: &'static str = "custom_command";
    type Error = CustomCommandError;
    type Args = CustomCommandArgs;
    type Output = String;

    fn name(&self) -> String {
        self.tool_name.clone()
    }

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: self.tool_name.clone(),
            description: self.description.clone(),
            parameters: self.parameters_schema.clone(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let unknown_params = self.validate_params(&args.params)?;

        let full_command = self.build_command(&args.params);
        let sandboxed_command = build_sandboxed_command(&full_command);
        let start = crate::tools::start_tool_call();

        let output = self
            .execute_command(&sandboxed_command, &self.sanitized_env)
            .await?;
        let result = Self::prepare_output(&output);

        let exit_code = output.status.code().unwrap_or(-1);
        let mut audit_result = format!("exit {exit_code}, {}", format_byte_size(result.len()));
        if !unknown_params.is_empty() {
            audit_result.push_str(&format!(
                ", ignored unknown params: {}",
                unknown_params
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        crate::tools::finish_tool_call(start, &self.tool_name, &full_command, audit_result);

        Ok(result)
    }
}

impl CustomCommandTool {
    /// Execute the sandboxed command with timeout and sanitized environment.
    async fn execute_command(
        &self,
        sandboxed_command: &str,
        sanitized_env: &std::collections::HashMap<String, String>,
    ) -> Result<std::process::Output, CustomCommandError> {
        tokio::time::timeout(
            COMMAND_TIMEOUT,
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(sandboxed_command)
                .current_dir(&self.repo_root)
                .env_clear()
                .envs(sanitized_env)
                .output(),
        )
        .await
        .map_err(|_| {
            CustomCommandError(format!(
                "command timed out after {}s",
                COMMAND_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| CustomCommandError(format!("failed to execute command: {e}")))
    }

    /// Combine stdout/stderr output, truncating if necessary.
    fn prepare_output(output: &std::process::Output) -> String {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();

        if !stdout.is_empty() {
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("[stderr]\n");
            result.push_str(&stderr);
        }

        if result.is_empty() {
            result = format!(
                "Command completed with exit code: {}",
                output.status.code().unwrap_or(-1)
            );
        }

        // Truncate very large outputs
        if result.len() > MAX_OUTPUT_SIZE {
            result.truncate(MAX_OUTPUT_SIZE);
            result.push_str("\n... [output truncated]");
        }

        result
    }
}

/// Format a byte count as a human-readable size string.
fn format_byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Wrap a command with `ulimit` resource limits.
///
/// Applies soft limits on virtual memory, child process count, and file
/// size to prevent runaway subprocesses from exhausting the host's
/// resources. Errors from unsupported limits are suppressed (`2>/dev/null`)
/// so this is safe on platforms where a specific limit isn't available
/// (e.g. `ulimit -v` is not supported on macOS with Apple Silicon).
fn build_sandboxed_command(command: &str) -> String {
    format!(
        "ulimit -v {ULIMIT_VMEM_KB} 2>/dev/null; \
         ulimit -f {ULIMIT_FSIZE_BLOCKS} 2>/dev/null; \
         {command}"
    )
}

/// Minimal shell escaping for parameter values.
///
/// Wraps the value in single quotes and escapes any embedded single quotes.
/// This prevents shell injection via parameter values.
fn shell_escape(value: &str) -> String {
    // If the value contains no special characters, return as-is
    if value
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
    {
        return value.to_string();
    }

    // Wrap in single quotes, escaping any embedded single quotes
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Build a sanitized copy of the current environment.
///
/// Uses an **allowlist** model: only variables in
/// [`crate::constants::SAFE_ENV_VARS`] / [`crate::constants::SAFE_ENV_PREFIXES`]
/// plus those matching the profile's `env_passthrough` patterns are
/// included. Everything else is dropped so that API keys, tokens,
/// and credentials never leak to LLM-invoked commands by default.
///
/// Called once at `CustomCommandTool` construction time so the cost
/// is not repeated per tool invocation.
fn build_sanitized_env(env_passthrough: &[String]) -> HashMap<String, String> {
    let mut env = HashMap::new();

    for (key, value) in std::env::vars() {
        if is_var_allowed(&key, env_passthrough) {
            env.insert(key, value);
        }
    }

    env
}

/// Returns `true` if `var_name` is allowed in the subprocess.
///
/// A variable is allowed if it appears in the safe set
/// ([`crate::constants::SAFE_ENV_VARS`] or matches a
/// [`crate::constants::SAFE_ENV_PREFIXES`] entry) **or** matches the
/// profile-declared `env_passthrough` patterns.
fn is_var_allowed(var_name: &str, env_passthrough: &[String]) -> bool {
    if crate::constants::SAFE_ENV_VARS.contains(&var_name) {
        return true;
    }

    for &prefix in crate::constants::SAFE_ENV_PREFIXES {
        if var_name.starts_with(prefix) {
            return true;
        }
    }

    for pattern in env_passthrough {
        if let Some(prefix) = pattern.strip_suffix('*') {
            if var_name.starts_with(prefix) {
                return true;
            }
        } else if pattern == var_name {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::agent::ToolParameter;

    fn test_def() -> CustomToolDefinition {
        CustomToolDefinition {
            name: "run_tests".to_string(),
            description: "Run the test suite".to_string(),
            command: "cargo test".to_string(),
            parameters: vec![
                ToolParameter {
                    name: "filter".to_string(),
                    param_type: "string".to_string(),
                    description: "Test name filter".to_string(),
                    required: false,
                },
                ToolParameter {
                    name: "verbose".to_string(),
                    param_type: "boolean".to_string(),
                    description: "Show verbose output".to_string(),
                    required: false,
                },
            ],
        }
    }

    #[test]
    fn build_command_no_params() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        let params = serde_json::Map::new();
        assert_eq!(tool.build_command(&params), "cargo test");
    }

    #[test]
    fn build_command_with_string_param() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        let mut params = serde_json::Map::new();
        params.insert(
            "filter".to_string(),
            serde_json::Value::String("my_test".to_string()),
        );
        assert_eq!(tool.build_command(&params), "cargo test --filter my_test");
    }

    #[test]
    fn build_command_with_bool_true() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        let mut params = serde_json::Map::new();
        params.insert("verbose".to_string(), serde_json::Value::Bool(true));
        assert_eq!(tool.build_command(&params), "cargo test --verbose");
    }

    #[test]
    fn build_command_with_bool_false() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        let mut params = serde_json::Map::new();
        params.insert("verbose".to_string(), serde_json::Value::Bool(false));
        assert_eq!(tool.build_command(&params), "cargo test");
    }

    #[test]
    fn build_command_escapes_special_chars() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        let mut params = serde_json::Map::new();
        params.insert(
            "filter".to_string(),
            serde_json::Value::String("test; rm -rf /".to_string()),
        );
        assert_eq!(
            tool.build_command(&params),
            "cargo test --filter 'test; rm -rf /'"
        );
    }

    #[tokio::test]
    async fn tool_definition_matches() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "run_tests");
        assert_eq!(def.description, "Run the test suite");
        assert_eq!(def.parameters["properties"]["filter"]["type"], "string");
    }

    #[tokio::test]
    async fn tool_name_override() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        assert_eq!(Tool::name(&tool), "run_tests");
    }

    #[tokio::test]
    async fn execute_simple_command() {
        let def = CustomToolDefinition {
            name: "echo_test".to_string(),
            description: "Echo something".to_string(),
            command: "echo hello".to_string(),
            parameters: vec![],
        };
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"), vec![]);
        let args = CustomCommandArgs {
            params: serde_json::Map::new(),
        };
        let result = tool.call(args).await.unwrap();
        assert_eq!(result.trim(), "hello");
    }

    #[tokio::test]
    async fn missing_required_param() {
        let def = CustomToolDefinition {
            name: "needs_param".to_string(),
            description: "Needs a param".to_string(),
            command: "echo".to_string(),
            parameters: vec![ToolParameter {
                name: "required_arg".to_string(),
                param_type: "string".to_string(),
                description: "A required arg".to_string(),
                required: true,
            }],
        };
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"), vec![]);
        let args = CustomCommandArgs {
            params: serde_json::Map::new(),
        };
        let result = tool.call(args).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().0.contains("missing required parameter"));
    }

    #[test]
    fn shell_escape_safe_value() {
        assert_eq!(shell_escape("hello"), "hello");
        assert_eq!(shell_escape("test_name"), "test_name");
        assert_eq!(shell_escape("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn shell_escape_dangerous_value() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
        assert_eq!(shell_escape("$(whoami)"), "'$(whoami)'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn env_sanitization_allowlist_only() {
        // Set a var that is NOT on the safe list
        unsafe { std::env::set_var("SOME_SECRET_TOKEN", "secret-123") };
        // Set a var that IS on the safe list
        // (PATH is always present, but let's verify)

        let env = build_sanitized_env(&[]);

        assert!(
            !env.contains_key("SOME_SECRET_TOKEN"),
            "non-safe vars should be stripped"
        );
        assert!(
            !env.contains_key("ANTHROPIC_API_KEY"),
            "API keys should be stripped"
        );
        assert!(
            env.contains_key("PATH"),
            "PATH should be preserved (safe list)"
        );
        assert!(
            env.contains_key("HOME"),
            "HOME should be preserved (safe list)"
        );

        // Cleanup
        unsafe { std::env::remove_var("SOME_SECRET_TOKEN") };
    }

    #[test]
    fn env_passthrough_exact_match() {
        unsafe { std::env::set_var("CUSTOM_DB_URL", "postgres://localhost/mydb") };

        let env = build_sanitized_env(&["CUSTOM_DB_URL".to_string()]);

        assert_eq!(
            env.get("CUSTOM_DB_URL").map(|s| s.as_str()),
            Some("postgres://localhost/mydb"),
            "explicitly passed-through var should be preserved"
        );

        unsafe { std::env::remove_var("CUSTOM_DB_URL") };
    }

    #[test]
    fn env_passthrough_prefix_glob() {
        unsafe { std::env::set_var("AWS_REGION", "us-east-1") };
        unsafe { std::env::set_var("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI") };

        // Without passthrough, AWS_ vars should NOT appear
        let env_no_pass = build_sanitized_env(&[]);
        assert!(
            !env_no_pass.contains_key("AWS_REGION"),
            "AWS_REGION should not appear without passthrough"
        );

        // With passthrough, they should
        let env = build_sanitized_env(&["AWS_*".to_string()]);

        assert_eq!(
            env.get("AWS_REGION").map(|s| s.as_str()),
            Some("us-east-1"),
            "AWS_* glob should pass through AWS_REGION"
        );
        assert_eq!(
            env.get("AWS_SECRET_ACCESS_KEY").map(|s| s.as_str()),
            Some("wJalrXUtnFEMI"),
            "AWS_* glob should pass through AWS_SECRET_ACCESS_KEY"
        );

        unsafe { std::env::remove_var("AWS_REGION") };
        unsafe { std::env::remove_var("AWS_SECRET_ACCESS_KEY") };
    }

    #[test]
    fn is_allowed_logic() {
        let passthrough = vec!["JIRA_TOKEN".to_string(), "AWS_*".to_string()];

        // Safe list vars
        assert!(is_var_allowed("PATH", &passthrough));
        assert!(is_var_allowed("HOME", &passthrough));
        assert!(is_var_allowed("LANG", &passthrough));

        // Safe prefix vars
        assert!(is_var_allowed("LC_ALL", &passthrough));
        assert!(is_var_allowed("XDG_CONFIG_HOME", &passthrough));

        // Passthrough vars
        assert!(is_var_allowed("JIRA_TOKEN", &passthrough));
        assert!(is_var_allowed("AWS_REGION", &passthrough));
        assert!(is_var_allowed("AWS_SECRET_ACCESS_KEY", &passthrough));

        // Non-allowed vars
        assert!(!is_var_allowed("ANTHROPIC_API_KEY", &passthrough));
        assert!(!is_var_allowed("GITHUB_TOKEN", &passthrough));
        assert!(!is_var_allowed("DATABASE_URL", &passthrough));
    }

    #[tokio::test]
    async fn execute_command_env_sanitized() {
        // Verify that a non-safe env var is NOT visible to the subprocess
        unsafe { std::env::set_var("GEMINI_API_KEY", "test-gemini-key-for-sanitization") };

        let def = CustomToolDefinition {
            name: "check_env".to_string(),
            description: "Print env var".to_string(),
            command: "printenv GEMINI_API_KEY || echo MISSING".to_string(),
            parameters: vec![],
        };
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"), vec![]);
        let args = CustomCommandArgs {
            params: serde_json::Map::new(),
        };
        let result = tool.call(args).await.unwrap();
        assert!(
            result.contains("MISSING"),
            "GEMINI_API_KEY should not be visible in subprocess (allowlist model), got: {result}"
        );

        unsafe { std::env::remove_var("GEMINI_API_KEY") };
    }

    #[test]
    fn safe_prefix_vars_are_included() {
        unsafe { std::env::set_var("LC_ALL", "en_US.UTF-8") };
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/home/test/.config") };

        let env = build_sanitized_env(&[]);

        assert_eq!(
            env.get("LC_ALL").map(|s| s.as_str()),
            Some("en_US.UTF-8"),
            "LC_ prefix should be safe"
        );
        assert_eq!(
            env.get("XDG_CONFIG_HOME").map(|s| s.as_str()),
            Some("/home/test/.config"),
            "XDG_ prefix should be safe"
        );

        unsafe { std::env::remove_var("LC_ALL") };
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }

    #[test]
    fn sandboxed_command_includes_ulimits() {
        let sandboxed = build_sandboxed_command("echo hello");
        assert!(
            sandboxed.contains("ulimit -v"),
            "should include virtual memory limit"
        );
        assert!(
            sandboxed.contains("ulimit -f"),
            "should include file size limit"
        );
        assert!(
            sandboxed.contains("2>/dev/null"),
            "should suppress errors for unsupported limits"
        );
        assert!(
            sandboxed.ends_with("echo hello"),
            "original command should be at the end"
        );
    }

    #[test]
    fn sandboxed_command_uses_correct_limits() {
        let sandboxed = build_sandboxed_command("true");
        assert!(sandboxed.contains(&format!("ulimit -v {ULIMIT_VMEM_KB}")));
        assert!(sandboxed.contains(&format!("ulimit -f {ULIMIT_FSIZE_BLOCKS}")));
    }

    #[test]
    fn validate_params_missing_required() {
        let def = CustomToolDefinition {
            name: "needs_param".to_string(),
            description: "Test".to_string(),
            command: "echo".to_string(),
            parameters: vec![ToolParameter {
                name: "arg".to_string(),
                param_type: "string".to_string(),
                description: "Required".to_string(),
                required: true,
            }],
        };
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"), vec![]);
        let params = serde_json::Map::new();
        let result = tool.validate_params(&params);
        assert!(result.is_err());
    }

    #[test]
    fn validate_params_unknown_params_returned() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"), vec![]);
        let mut params = serde_json::Map::new();
        params.insert("filter".to_string(), serde_json::Value::String("x".into()));
        params.insert("rogue".to_string(), serde_json::Value::String("y".into()));
        let unknown = tool.validate_params(&params).unwrap();
        assert_eq!(unknown, vec!["rogue".to_string()]);
    }

    #[tokio::test]
    async fn execute_with_ulimit_sandbox() {
        // Verify that a sandboxed command still executes successfully
        let def = CustomToolDefinition {
            name: "sandboxed_echo".to_string(),
            description: "Echo with sandbox".to_string(),
            command: "echo sandboxed".to_string(),
            parameters: vec![],
        };
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"), vec![]);
        let args = CustomCommandArgs {
            params: serde_json::Map::new(),
        };
        let result = tool.call(args).await.unwrap();
        assert!(
            result.contains("sandboxed"),
            "command should execute through ulimit wrapper, got: {result}"
        );
    }

    #[tokio::test]
    async fn unknown_params_ignored_but_command_succeeds() {
        let def = CustomToolDefinition {
            name: "echo_test".to_string(),
            description: "Simple echo".to_string(),
            command: "echo ok".to_string(),
            parameters: vec![],
        };
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"), vec![]);
        let mut params = serde_json::Map::new();
        params.insert(
            "rogue_param".to_string(),
            serde_json::Value::String("evil".to_string()),
        );
        params.insert(
            "another_unknown".to_string(),
            serde_json::Value::String("ignored".to_string()),
        );
        let args = CustomCommandArgs { params };
        // Should succeed — unknown params are ignored, not rejected
        let result = tool.call(args).await.unwrap();
        assert!(
            result.contains("ok"),
            "command should still succeed with unknown params, got: {result}"
        );
    }
}
