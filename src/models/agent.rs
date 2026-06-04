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
    /// Tags for categorization and selection via `--tag`.
    ///
    /// When `--tag` is passed on the CLI, all loaded profiles whose tags
    /// contain any of the requested values are included in the review.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Custom tool definitions for agentic mode.
    #[serde(default)]
    pub tools: Vec<CustomToolDefinition>,
    /// Optional profile-specific instructions for agentic mode.
    ///
    /// When present, these are appended to the agentic system prompt
    /// to give profile-specific tool-usage guidance (e.g., "use
    /// `search_text` to trace tainted data flow").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agentic_instructions: Option<String>,
    /// Environment variable names (or prefixes ending with `*`) that
    /// custom command tools are allowed to inherit.
    ///
    /// By default, nitpik strips all sensitive env vars (LLM API keys,
    /// license key) from the subprocess environment. Variables listed
    /// here are re-injected from the parent process so that custom
    /// tools can authenticate against external services.
    ///
    /// Example:
    /// ```yaml
    /// environment:
    ///   - JIRA_TOKEN
    ///   - AWS_*
    /// ```
    #[serde(default)]
    pub environment: Vec<String>,
    /// Whether this profile is automatically included in every `auto`
    /// review, regardless of the file-classification heuristics.
    ///
    /// Use this for cross-cutting reviewers that should always run —
    /// for example a security reviewer or a documentation-drift
    /// reviewer. Has no effect when the user selects profiles
    /// explicitly via `--profile` or `--tag`.
    ///
    /// A custom profile in `--profile-dir` whose `name` matches a
    /// built-in replaces the built-in entirely (see
    /// [`agents::resolve_profiles`](crate::agents::resolve_profiles)),
    /// so a user can disable a built-in always-on profile by shipping
    /// an override with `always_include: false`.
    #[serde(default)]
    pub always_include: bool,

    /// Whether this profile is for nitpik's own internal passes (the
    /// verify critic, the auto-selection triage) rather than a reviewer
    /// the user runs. Internal profiles are excluded from listings, tag
    /// matching, `auto` always-include, and explicit `--profile`
    /// selection; nitpik loads them by name through dedicated internal
    /// code paths only (see
    /// [`agents::builtin::get_builtin`](crate::agents::builtin::get_builtin)).
    ///
    /// A custom profile of the same name shipped in `--profile-dir`
    /// replaces the built-in entirely, so overriding `critic`/`triage`
    /// without `internal: true` deliberately makes that name
    /// user-selectable.
    #[serde(default, skip_serializing_if = "is_false")]
    pub internal: bool,

    /// Wave this profile runs in for multi-wave reviews.
    ///
    /// Wave 1 is the default. Profiles with `wave: 2` run after wave 1
    /// completes and receive the wave-1 findings as additional context
    /// in their system prompt. Used to express dependencies between
    /// reviewers (e.g. an architect that wants to react to backend
    /// findings). Capped at 2 waves; values > 2 are treated as 2.
    ///
    /// Multi-wave dispatch is only active when the user opts in via
    /// `--multi-wave`; otherwise every profile runs in a single wave
    /// regardless of this field.
    #[serde(default = "default_wave")]
    pub wave: u8,

    /// What slice of the change this reviewer sees per task.
    ///
    /// `chunk` (default) runs the reviewer once per diff chunk (the
    /// surrounding file plus the chunk) — cheap, parallel, the bulk of
    /// review. `diff` runs it once over the *whole* change set, for
    /// cross-cutting concerns (impact/rename tracing, contract changes,
    /// symmetric-obligation checks) that a single chunk can't reveal.
    ///
    /// This is the per-lens half of the lens model; see
    /// `plans/pr-native-review/lens-model.md`.
    #[serde(default)]
    pub scope: LensScope,

    /// Whether this reviewer wants repository-exploration tools (agentic
    /// mode) by default.
    ///
    /// Expresses the reviewer's *intent*: cheap local lenses leave this
    /// `false`; cross-cutting lenses that must read beyond the diff set
    /// it `true`. The run-level `--agent` policy can still force agentic
    /// mode on or off across every reviewer, overriding this default.
    #[serde(default)]
    pub agentic: bool,
}

fn default_wave() -> u8 {
    1
}

/// The slice of a change set a reviewer operates on per task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LensScope {
    /// One task per diff chunk (chunk + surrounding file). The default —
    /// cheap, parallel, and how every reviewer has always run.
    #[default]
    Chunk,
    /// One task over the entire change set, for cross-cutting concerns.
    Diff,
}

/// Run-level policy for agentic (tool-using) review, layered on top of each
/// reviewer's own [`agentic`](AgentProfile::agentic) intent.
///
/// `--agent` on the CLI selects this; absent, it defaults to [`Auto`].
///
/// [`Auto`]: AgentPolicy::Auto
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentPolicy {
    /// Honor each reviewer's declared `agentic` field — cheap local lenses
    /// stay single-shot, cross-cutting lenses use tools. The default.
    #[default]
    Auto,
    /// Force every reviewer agentic, overriding their declaration —
    /// maximum thoroughness, higher cost.
    On,
    /// Force every reviewer single-shot — caps cost/latency, even for
    /// lenses that would normally explore.
    Off,
}

impl AgentPolicy {
    /// Resolve the effective agentic mode for a reviewer given its declared
    /// per-lens intent.
    pub fn resolve(self, lens_agentic: bool) -> bool {
        match self {
            AgentPolicy::On => true,
            AgentPolicy::Off => false,
            AgentPolicy::Auto => lens_agentic,
        }
    }
}

/// Serde `skip_serializing_if` predicate: omit `false` booleans so
/// flags like `internal` don't add noise to serialized profile output.
fn is_false(b: &bool) -> bool {
    !*b
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_policy_default_is_auto() {
        assert_eq!(AgentPolicy::default(), AgentPolicy::Auto);
    }

    #[test]
    fn agent_policy_auto_honors_lens_intent() {
        assert!(AgentPolicy::Auto.resolve(true));
        assert!(!AgentPolicy::Auto.resolve(false));
    }

    #[test]
    fn agent_policy_on_forces_agentic() {
        assert!(AgentPolicy::On.resolve(false));
        assert!(AgentPolicy::On.resolve(true));
    }

    #[test]
    fn agent_policy_off_forces_single_shot() {
        assert!(!AgentPolicy::Off.resolve(true));
        assert!(!AgentPolicy::Off.resolve(false));
    }

    #[test]
    fn lens_scope_defaults_to_chunk() {
        assert_eq!(LensScope::default(), LensScope::Chunk);
    }
}
