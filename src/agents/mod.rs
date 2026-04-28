//! Agent loading, profile resolution, and markdown+YAML parsing.
//!
//! # Bounded Context: Reviewer Profiles
//!
//! Owns profile parsing (YAML frontmatter + markdown body), built-in
//! profile registry, tag-based auto-selection, and profile resolution
//! from CLI flags. Produces [`AgentDefinition`](crate::models::agent::AgentDefinition)
//! values consumed by the orchestrator — never calls the LLM directly.

pub mod auto;
pub mod builtin;
pub mod parser;

use std::path::Path;
use thiserror::Error;

use crate::models::AgentDefinition;

/// Errors from agent loading.
#[derive(Error, Debug)]
pub enum AgentError {
    #[error("agent profile not found: {0}")]
    NotFound(String),

    #[error("failed to read agent file {path}: {source}")]
    ReadError {
        path: String,
        source: std::io::Error,
    },

    #[error("failed to parse agent definition: {0}")]
    ParseError(String),
}

/// Resolve a list of profile names/paths into agent definitions.
///
/// Resolution order for each value:
/// 1. If `agent_dir` is set and `{agent_dir}/{value}.md` exists → load it
///    (custom profiles override built-ins with the same name)
/// 2. If it matches a built-in name → use embedded profile
/// 3. If it's a file path (contains `/` or `.md`) → load it directly
/// 4. Otherwise → error with suggestions
pub async fn resolve_profiles(
    profiles: &[String],
    agent_dir: Option<&Path>,
) -> Result<Vec<AgentDefinition>, AgentError> {
    let mut agents = Vec::new();

    for profile in profiles {
        let agent = resolve_single_profile(profile, agent_dir).await?;
        agents.push(agent);
    }

    Ok(agents)
}

/// List all available profiles: built-ins plus any custom ones from `agent_dir`.
///
/// Custom profiles whose `name` matches a built-in replace the built-in entry.
pub async fn list_all_profiles(
    agent_dir: Option<&Path>,
) -> Result<Vec<AgentDefinition>, AgentError> {
    let mut agents: Vec<AgentDefinition> = Vec::new();

    // Custom profiles from agent_dir take precedence — load them first
    // so we know which built-in names to skip.
    let mut custom_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(dir) = agent_dir {
        if dir.is_dir() {
            let mut entries =
                tokio::fs::read_dir(dir)
                    .await
                    .map_err(|e| AgentError::ReadError {
                        path: dir.display().to_string(),
                        source: e,
                    })?;

            while let Some(entry) =
                entries
                    .next_entry()
                    .await
                    .map_err(|e| AgentError::ReadError {
                        path: dir.display().to_string(),
                        source: e,
                    })?
            {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md") {
                    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                        AgentError::ReadError {
                            path: path.display().to_string(),
                            source: e,
                        }
                    })?;
                    match parser::parse_agent_definition(&content) {
                        Ok(agent) => {
                            custom_names.insert(agent.profile.name.clone());
                            agents.push(agent);
                        }
                        Err(e) => {
                            eprintln!("Warning: skipping {}: {e}", path.display());
                        }
                    }
                }
            }
        }
    }

    // Built-in profiles, skipping any overridden by a custom profile of the same name.
    for name in builtin::list_builtin_names() {
        if custom_names.contains(name) {
            continue;
        }
        if let Some(agent) = builtin::get_builtin(name) {
            agents.push(agent);
        }
    }

    Ok(agents)
}

/// Resolve profiles whose tags match any of the given tag values.
///
/// Loads all available profiles (built-in + custom from `agent_dir`), then
/// returns those that contain at least one of the requested tags.
/// Tag matching is case-insensitive.
pub async fn resolve_profiles_by_tags(
    tags: &[String],
    agent_dir: Option<&Path>,
) -> Result<Vec<AgentDefinition>, AgentError> {
    let all = list_all_profiles(agent_dir).await?;
    let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();

    let matched: Vec<AgentDefinition> = all
        .into_iter()
        .filter(|agent| {
            agent
                .profile
                .tags
                .iter()
                .any(|t| lower_tags.contains(&t.to_lowercase()))
        })
        .collect();

    Ok(matched)
}

/// Resolve a single profile name or path.
async fn resolve_single_profile(
    profile: &str,
    agent_dir: Option<&Path>,
) -> Result<AgentDefinition, AgentError> {
    // 1. Check agent_dir first so custom profiles can override built-ins
    if let Some(dir) = agent_dir {
        let path = dir.join(format!("{profile}.md"));
        if path.exists() {
            let content =
                tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|e| AgentError::ReadError {
                        path: path.display().to_string(),
                        source: e,
                    })?;
            return parser::parse_agent_definition(&content)
                .map_err(|e| AgentError::ParseError(e.to_string()));
        }
    }

    // 2. Check built-in profiles
    if let Some(agent) = builtin::get_builtin(profile) {
        return Ok(agent);
    }

    // 3. Check if it's a direct file path
    if profile.contains('/') || profile.ends_with(".md") {
        let path = Path::new(profile);
        if path.exists() {
            let content =
                tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| AgentError::ReadError {
                        path: profile.to_string(),
                        source: e,
                    })?;
            return parser::parse_agent_definition(&content)
                .map_err(|e| AgentError::ParseError(e.to_string()));
        }
        return Err(AgentError::NotFound(format!("file not found: {profile}")));
    }

    // 4. Error with suggestions
    let builtins = builtin::list_builtin_names();
    Err(AgentError::NotFound(format!(
        "unknown profile '{profile}'. Available built-in profiles: {}",
        builtins.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_builtin_profile() {
        let agents = resolve_profiles(&["backend".to_string()], None)
            .await
            .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].profile.name, "backend");
    }

    #[tokio::test]
    async fn resolve_multiple_builtins() {
        let profiles = vec!["backend".to_string(), "security".to_string()];
        let agents = resolve_profiles(&profiles, None).await.unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].profile.name, "backend");
        assert_eq!(agents[1].profile.name, "security");
    }

    #[tokio::test]
    async fn resolve_unknown_profile_errors() {
        let result = resolve_profiles(&["nonexistent".to_string()], None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown profile"), "got: {err}");
        assert!(
            err.contains("backend"),
            "should suggest built-ins, got: {err}"
        );
    }

    #[tokio::test]
    async fn resolve_from_agent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let agent_file = dir.path().join("custom.md");
        std::fs::write(
            &agent_file,
            "---\nname: custom\ndescription: A custom agent\ntags: []\n---\nYou are a custom reviewer.",
        )
        .unwrap();

        let agents = resolve_profiles(&["custom".to_string()], Some(dir.path()))
            .await
            .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].profile.name, "custom");
    }

    #[tokio::test]
    async fn resolve_direct_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let agent_file = dir.path().join("my_agent.md");
        std::fs::write(
            &agent_file,
            "---\nname: my_agent\ndescription: Direct path agent\ntags: []\n---\nSystem prompt.",
        )
        .unwrap();

        let path_str = agent_file.display().to_string();
        let agents = resolve_profiles(&[path_str], None).await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].profile.name, "my_agent");
    }

    #[tokio::test]
    async fn resolve_file_not_found() {
        let result = resolve_profiles(&["/tmp/nitpik_no_such_file.md".to_string()], None).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn resolve_agent_dir_miss_falls_through() {
        let dir = tempfile::tempdir().unwrap();
        // agent_dir exists but doesn't contain "missing.md"
        let result = resolve_profiles(&["missing".to_string()], Some(dir.path())).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown profile"), "got: {err}");
    }

    #[tokio::test]
    async fn list_all_builtins_without_agent_dir() {
        let agents = list_all_profiles(None).await.unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(names.contains(&"backend"));
        assert!(names.contains(&"frontend"));
        assert!(names.contains(&"architect"));
        assert!(names.contains(&"security"));
        assert_eq!(agents.len(), 4);
    }

    #[tokio::test]
    async fn list_all_includes_custom_profiles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("custom.md"),
            "---\nname: custom\ndescription: Custom\ntags: []\n---\nPrompt.",
        )
        .unwrap();
        // Non-.md files should be ignored
        std::fs::write(dir.path().join("readme.txt"), "not a profile").unwrap();

        let agents = list_all_profiles(Some(dir.path())).await.unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(names.contains(&"backend"));
        assert!(names.contains(&"custom"));
        assert_eq!(agents.len(), 5);
    }

    #[tokio::test]
    async fn list_all_skips_invalid_custom_profiles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.md"), "no frontmatter").unwrap();

        let agents = list_all_profiles(Some(dir.path())).await.unwrap();
        // Only built-ins, bad.md skipped with warning
        assert_eq!(agents.len(), 4);
    }

    #[tokio::test]
    async fn list_all_nonexistent_agent_dir() {
        let result =
            list_all_profiles(Some(std::path::Path::new("/tmp/nitpik_no_such_dir_xyz"))).await;
        // Non-existent dir is not an error — it's just not a directory, so skip
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 4); // just built-ins
    }

    // -----------------------------------------------------------------------
    // resolve_profiles_by_tags
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_by_tag_matches_builtin() {
        // "backend" is a tag on the backend profile
        let agents = resolve_profiles_by_tags(&["backend".to_string()], None)
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(names.contains(&"backend"), "got: {names:?}");
    }

    #[tokio::test]
    async fn resolve_by_tag_matches_multiple_profiles() {
        // "security" is a tag on the security profile; "performance" is on backend
        let agents =
            resolve_profiles_by_tags(&["security".to_string(), "performance".to_string()], None)
                .await
                .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(
            names.contains(&"backend"),
            "performance tag → backend; got: {names:?}"
        );
        assert!(
            names.contains(&"security"),
            "security tag → security; got: {names:?}"
        );
    }

    #[tokio::test]
    async fn resolve_by_tag_is_case_insensitive() {
        let agents = resolve_profiles_by_tags(&["BACKEND".to_string()], None)
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(
            names.contains(&"backend"),
            "case-insensitive match; got: {names:?}"
        );
    }

    #[tokio::test]
    async fn resolve_by_tag_no_match_returns_empty() {
        let agents = resolve_profiles_by_tags(&["nonexistent-tag".to_string()], None)
            .await
            .unwrap();
        assert!(agents.is_empty(), "should return empty for unknown tag");
    }

    #[tokio::test]
    async fn resolve_by_tag_includes_custom_profiles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("custom.md"),
            "---\nname: custom\ndescription: Custom\ntags: [my-tag, css]\n---\nPrompt.",
        )
        .unwrap();

        let agents = resolve_profiles_by_tags(&["my-tag".to_string()], Some(dir.path()))
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["custom"],
            "only custom has my-tag; got: {names:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Custom profile overrides built-in
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_custom_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("backend.md"),
            "---\nname: backend\ndescription: My override\ntags: [custom-tag]\n---\nOverridden prompt body.",
        )
        .unwrap();

        let agents = resolve_profiles(&["backend".to_string()], Some(dir.path()))
            .await
            .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].profile.name, "backend");
        assert_eq!(
            agents[0].profile.description, "My override",
            "should load custom profile, not built-in"
        );
        assert_eq!(agents[0].profile.tags, vec!["custom-tag".to_string()]);
        assert!(agents[0].system_prompt.contains("Overridden prompt body"));
    }

    #[tokio::test]
    async fn list_all_custom_replaces_builtin_with_same_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("backend.md"),
            "---\nname: backend\ndescription: My override\ntags: []\n---\nOverridden prompt.",
        )
        .unwrap();

        let agents = list_all_profiles(Some(dir.path())).await.unwrap();
        // No duplicate "backend" entry
        let backend_entries: Vec<_> = agents
            .iter()
            .filter(|a| a.profile.name == "backend")
            .collect();
        assert_eq!(
            backend_entries.len(),
            1,
            "should not duplicate overridden profile"
        );
        assert_eq!(backend_entries[0].profile.description, "My override");
        // Other built-ins still present
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(names.contains(&"frontend"));
        assert!(names.contains(&"architect"));
        assert!(names.contains(&"security"));
        assert_eq!(agents.len(), 4);
    }

    #[tokio::test]
    async fn resolve_by_tag_uses_overridden_profile_tags() {
        // Built-in `backend` has tags like "backend", "performance".
        // Override removes those tags, so the built-in's tags should NOT match.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("backend.md"),
            "---\nname: backend\ndescription: Override\ntags: [only-mine]\n---\nPrompt.",
        )
        .unwrap();

        // The built-in's "performance" tag should no longer match because
        // the overridden profile replaces it.
        let agents = resolve_profiles_by_tags(&["performance".to_string()], Some(dir.path()))
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(
            !names.contains(&"backend"),
            "overridden backend lost 'performance' tag; got: {names:?}"
        );

        // The new tag matches and selects the override.
        let agents = resolve_profiles_by_tags(&["only-mine".to_string()], Some(dir.path()))
            .await
            .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].profile.name, "backend");
        assert_eq!(agents[0].profile.description, "Override");
    }

    #[tokio::test]
    async fn resolve_by_tag_shared_tag_selects_multiple() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("design.md"),
            "---\nname: design-system\ndescription: Design\ntags: [css, design]\n---\nPrompt.",
        )
        .unwrap();

        // "css" is on the built-in frontend AND the custom design-system profile
        let agents = resolve_profiles_by_tags(&["css".to_string()], Some(dir.path()))
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(
            names.contains(&"frontend"),
            "frontend has css tag; got: {names:?}"
        );
        assert!(
            names.contains(&"design-system"),
            "custom has css tag; got: {names:?}"
        );
    }
}
