//! Markdown + YAML frontmatter parser for agent definitions.
//!
//! Uses `serde_yaml_ng` for proper YAML parsing, which supports the nested
//! structures needed for custom tool definitions in agent profiles.

use crate::models::agent::{AgentDefinition, AgentProfile};

/// Parse a markdown file with YAML frontmatter into an AgentDefinition.
///
/// Expected format:
/// ```markdown
/// ---
/// name: my-agent
/// description: Does things
/// tools:
///   - name: run_tests
///     description: Run the test suite
///     command: cargo test
///     parameters:
///       - name: filter
///         type: string
///         description: Test name filter
///         required: false
/// ---
///
/// System prompt content here...
/// ```
pub fn parse_agent_definition(content: &str) -> Result<AgentDefinition, String> {
    let (frontmatter, body) = split_frontmatter(content)?;
    let profile: AgentProfile =
        serde_yaml_ng::from_str(&frontmatter).map_err(|e| format!("invalid frontmatter: {e}"))?;

    Ok(AgentDefinition {
        profile,
        system_prompt: body.trim().to_string(),
    })
}

/// Split content into YAML frontmatter and markdown body.
fn split_frontmatter(content: &str) -> Result<(String, String), String> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Err("agent definition must start with YAML frontmatter (---)".to_string());
    }

    let after_first = &content[3..];
    let end = after_first
        .find("\n---")
        .ok_or_else(|| "unterminated YAML frontmatter (missing closing ---)".to_string())?;

    let frontmatter = after_first[..end].trim().to_string();
    let body = after_first[end + 4..].to_string();

    Ok((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_agent() {
        let content = r#"---
name: security
description: Focuses on security vulnerabilities
model: claude-sonnet-4-20250514
tags: [security, auth, injection]
---

You are a senior security engineer performing a code review.

Focus on injection vulnerabilities and auth issues.
"#;
        let agent = parse_agent_definition(content).unwrap();
        assert_eq!(agent.profile.name, "security");
        assert_eq!(
            agent.profile.description,
            "Focuses on security vulnerabilities"
        );
        assert_eq!(
            agent.profile.model,
            Some("claude-sonnet-4-20250514".to_string())
        );
        assert_eq!(agent.profile.tags, vec!["security", "auth", "injection"]);
        assert!(agent.system_prompt.starts_with("You are a senior"));
    }

    #[test]
    fn parse_minimal_agent() {
        let content = r#"---
name: test
description: A test agent
---

Do things."#;
        let agent = parse_agent_definition(content).unwrap();
        assert_eq!(agent.profile.name, "test");
        assert!(agent.profile.model.is_none());
        assert!(agent.profile.tags.is_empty());
        assert!(agent.profile.tools.is_empty());
    }

    #[test]
    fn missing_frontmatter() {
        let result = parse_agent_definition("No frontmatter here");
        assert!(result.is_err());
    }

    #[test]
    fn missing_name() {
        let content = r#"---
description: No name
---
Prompt."#;
        let result = parse_agent_definition(content);
        assert!(result.is_err());
    }

    #[test]
    fn parse_agent_with_custom_tools() {
        let content = r#"---
name: rust-reviewer
description: Rust code reviewer with custom checks
tools:
  - name: run_tests
    description: Run the project's test suite
    command: cargo test
    parameters:
      - name: filter
        type: string
        description: Optional test name filter
        required: false
  - name: check_lints
    description: Run clippy linter on the codebase
    command: cargo clippy -- -D warnings
---

You are a Rust code reviewer."#;
        let agent = parse_agent_definition(content).unwrap();
        assert_eq!(agent.profile.name, "rust-reviewer");
        assert_eq!(agent.profile.tools.len(), 2);

        let tool1 = &agent.profile.tools[0];
        assert_eq!(tool1.name, "run_tests");
        assert_eq!(tool1.command, "cargo test");
        assert_eq!(tool1.parameters.len(), 1);
        assert_eq!(tool1.parameters[0].name, "filter");
        assert_eq!(tool1.parameters[0].param_type, "string");
        assert!(!tool1.parameters[0].required);

        let tool2 = &agent.profile.tools[1];
        assert_eq!(tool2.name, "check_lints");
        assert_eq!(tool2.command, "cargo clippy -- -D warnings");
        assert!(tool2.parameters.is_empty());
    }

    #[test]
    fn parse_tool_with_required_params() {
        let content = r#"---
name: db-checker
description: Database reviewer
tools:
  - name: query_schema
    description: Query the database schema
    command: psql -c
    parameters:
      - name: table
        type: string
        description: Table name to inspect
        required: true
      - name: verbose
        type: boolean
        description: Show detailed output
        required: false
---

Review database code."#;
        let agent = parse_agent_definition(content).unwrap();
        assert_eq!(agent.profile.tools.len(), 1);

        let tool = &agent.profile.tools[0];
        assert_eq!(tool.parameters.len(), 2);
        assert!(tool.parameters[0].required);
        assert!(!tool.parameters[1].required);
        assert_eq!(tool.parameters[1].param_type, "boolean");
    }

    #[test]
    fn parse_agent_with_agentic_instructions() {
        let content = r#"---
name: security
description: Security reviewer
agentic_instructions: |
  Use search_text to trace data flow from user input to sinks.
  Use read_file to inspect sanitization helpers.
---

You are a security reviewer."#;
        let agent = parse_agent_definition(content).unwrap();
        assert_eq!(
            agent
                .profile
                .agentic_instructions
                .as_deref()
                .unwrap()
                .trim(),
            "Use search_text to trace data flow from user input to sinks.\nUse read_file to inspect sanitization helpers."
        );
        // agentic_instructions should NOT leak into the system prompt body
        assert!(!agent.system_prompt.contains("search_text"));
    }

    #[test]
    fn parse_agent_without_agentic_instructions() {
        let content = r#"---
name: basic
description: Basic reviewer
---

Review this code."#;
        let agent = parse_agent_definition(content).unwrap();
        assert!(agent.profile.agentic_instructions.is_none());
    }

    #[test]
    fn parse_agent_with_environment_passthrough() {
        let content = r#"---
name: infra-reviewer
description: Infrastructure reviewer
environment:
  - JIRA_TOKEN
  - AWS_*
  - DOCKER_HOST
tools:
  - name: deploy_check
    description: Check deployment status
    command: curl -s $JIRA_TOKEN/status
---

You are an infrastructure reviewer."#;
        let agent = parse_agent_definition(content).unwrap();
        assert_eq!(agent.profile.environment.len(), 3);
        assert_eq!(agent.profile.environment[0], "JIRA_TOKEN");
        assert_eq!(agent.profile.environment[1], "AWS_*");
        assert_eq!(agent.profile.environment[2], "DOCKER_HOST");
    }

    #[test]
    fn parse_agent_without_environment_defaults_empty() {
        let content = r#"---
name: basic
description: Basic reviewer
---

Review this code."#;
        let agent = parse_agent_definition(content).unwrap();
        assert!(agent.profile.environment.is_empty());
    }
}
