//! CustomCommandTool — execute user-defined CLI commands as agentic tools.
//!
//! This tool is constructed from a [`CustomToolDefinition`] in an agent
//! profile's YAML frontmatter. When the LLM invokes it, the specified
//! command is run as a subprocess (sandboxed to the repo root) and the
//! combined stdout/stderr is returned.

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
}

impl CustomCommandTool {
    /// Create a new `CustomCommandTool` from a profile definition and repo root.
    pub fn new(def: &CustomToolDefinition, repo_root: PathBuf) -> Self {
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

        Self {
            tool_name: def.name.clone(),
            description: def.description.clone(),
            command: def.command.clone(),
            parameters_schema,
            required_params: required.clone(),
            all_param_names: all_names,
            repo_root,
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
        // Validate required parameters
        for required in &self.required_params {
            if !args.params.contains_key(required) {
                return Err(CustomCommandError(format!(
                    "missing required parameter: {required}"
                )));
            }
        }

        let full_command = self.build_command(&args.params);

        // Execute via shell to support pipes, redirects, etc.
        let output = tokio::time::timeout(
            COMMAND_TIMEOUT,
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&full_command)
                .current_dir(&self.repo_root)
                .output(),
        )
        .await
        .map_err(|_| {
            CustomCommandError(format!(
                "command timed out after {}s: {full_command}",
                COMMAND_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| CustomCommandError(format!("failed to execute command: {e}")))?;

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

        Ok(result)
    }
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
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"));
        let params = serde_json::Map::new();
        assert_eq!(tool.build_command(&params), "cargo test");
    }

    #[test]
    fn build_command_with_string_param() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"));
        let mut params = serde_json::Map::new();
        params.insert(
            "filter".to_string(),
            serde_json::Value::String("my_test".to_string()),
        );
        assert_eq!(tool.build_command(&params), "cargo test --filter my_test");
    }

    #[test]
    fn build_command_with_bool_true() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"));
        let mut params = serde_json::Map::new();
        params.insert("verbose".to_string(), serde_json::Value::Bool(true));
        assert_eq!(tool.build_command(&params), "cargo test --verbose");
    }

    #[test]
    fn build_command_with_bool_false() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"));
        let mut params = serde_json::Map::new();
        params.insert("verbose".to_string(), serde_json::Value::Bool(false));
        assert_eq!(tool.build_command(&params), "cargo test");
    }

    #[test]
    fn build_command_escapes_special_chars() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"));
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
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"));
        let def = tool.definition(String::new()).await;
        assert_eq!(def.name, "run_tests");
        assert_eq!(def.description, "Run the test suite");
        assert_eq!(
            def.parameters["properties"]["filter"]["type"],
            "string"
        );
    }

    #[tokio::test]
    async fn tool_name_override() {
        let tool = CustomCommandTool::new(&test_def(), PathBuf::from("/tmp"));
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
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"));
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
        let tool = CustomCommandTool::new(&def, PathBuf::from("/tmp"));
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
}
