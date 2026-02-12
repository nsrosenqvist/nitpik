//! Agent definition types.

use serde::{Deserialize, Serialize};

/// A parsed agent profile from markdown+YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    /// Metadata from the YAML frontmatter.
    pub profile: AgentProfile,
    /// The system prompt (markdown body after frontmatter).
    pub system_prompt: String,
}

/// Metadata from the YAML frontmatter of an agent definition file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Unique name of the agent.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Optional model override for this agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tags for categorization and auto-selection.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Custom tool definitions for agentic mode.
    #[serde(default)]
    pub tools: Vec<CustomToolDefinition>,
}

/// A custom tool defined in agent profile frontmatter.
///
/// When the LLM invokes this tool, nitpik executes the specified command
/// as a subprocess, passing parameters as arguments or environment variables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolDefinition {
    /// Unique name for this tool (used by the LLM to call it).
    pub name: String,
    /// Human-readable description shown to the LLM.
    pub description: String,
    /// The command to execute (e.g. `cargo test`, `npm run lint`).
    pub command: String,
    /// Parameters the LLM can pass when invoking this tool.
    #[serde(default)]
    pub parameters: Vec<ToolParameter>,
}

/// A parameter for a custom tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameter {
    /// Parameter name.
    pub name: String,
    /// JSON Schema type (string, number, boolean, integer).
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,
    /// Human-readable description shown to the LLM.
    pub description: String,
    /// Whether this parameter is required.
    #[serde(default)]
    pub required: bool,
}

fn default_param_type() -> String {
    "string".to_string()
}
