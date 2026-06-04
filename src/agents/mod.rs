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
/// For each value, [`ProfileRepository::resolve`] is consulted:
/// 1. A registry name — a built-in, or a custom profile from `agent_dir`
///    that overrides a built-in of the same name. Internal profiles
///    (`critic`, `triage`) are *not* selectable this way.
/// 2. A file path (contains `/` or ends with `.md`) → loaded directly.
/// 3. Otherwise → error suggesting the selectable built-ins.
pub async fn resolve_profiles(
    profiles: &[String],
    agent_dir: Option<&Path>,
) -> Result<Vec<AgentDefinition>, AgentError> {
    let repo = ProfileRepository::load(agent_dir).await?;

    let mut agents = Vec::with_capacity(profiles.len());
    for profile in profiles {
        agents.push(repo.resolve(profile).await?);
    }

    Ok(agents)
}

/// List all user-selectable profiles: built-ins plus any custom ones
/// from `agent_dir`, excluding internal profiles (`critic`, `triage`).
///
/// Custom profiles whose `name` matches a built-in replace the built-in entry.
pub async fn list_all_profiles(
    agent_dir: Option<&Path>,
) -> Result<Vec<AgentDefinition>, AgentError> {
    let repo = ProfileRepository::load(agent_dir).await?;
    Ok(repo.selectable().cloned().collect())
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
    let repo = ProfileRepository::load(agent_dir).await?;
    let lower_tags: Vec<String> = tags.iter().map(|t| t.to_lowercase()).collect();

    Ok(repo
        .selectable()
        .filter(|agent| {
            agent
                .profile
                .tags
                .iter()
                .any(|t| lower_tags.contains(&t.to_lowercase()))
        })
        .cloned()
        .collect())
}

/// List profiles whose frontmatter declares `always_include: true`.
///
/// Loads all available profiles (built-in + custom from `agent_dir`,
/// with custom profiles overriding built-ins of the same name) and
/// returns those marked `always_include`. Used by the orchestrator to
/// append cross-cutting reviewers (e.g. the built-in `security`
/// profile, or a team's documentation-drift reviewer) to every `auto`
/// review.
///
/// A user can disable a built-in always-on profile by shipping an
/// override in `agent_dir` that sets `always_include: false`.
pub async fn list_always_include_profiles(
    agent_dir: Option<&Path>,
) -> Result<Vec<AgentDefinition>, AgentError> {
    let repo = ProfileRepository::load(agent_dir).await?;
    Ok(repo
        .selectable()
        .filter(|a| a.profile.always_include)
        .cloned()
        .collect())
}

/// The set of profiles available for one review run: every built-in plus
/// any custom profiles from `agent_dir`, with a custom profile replacing
/// the built-in of the same name.
///
/// This is the single load-and-resolve path. Every public helper
/// ([`resolve_profiles`], [`list_all_profiles`], [`resolve_profiles_by_tags`],
/// [`list_always_include_profiles`]) builds on it, so custom-override
/// precedence and internal-profile filtering live in exactly one place.
/// Internal profiles (`critic`, `triage`) are kept in the registry — so
/// resolution can tell "internal, not selectable" apart from "unknown" —
/// but are never returned by [`Self::selectable`]. nitpik's own passes
/// load them by name through [`builtin::get_builtin`], not through here.
struct ProfileRepository {
    /// Profiles in deterministic order: built-ins (declared order) first,
    /// then custom-only profiles (directory order). A custom override
    /// keeps the overridden built-in's position.
    profiles: Vec<AgentDefinition>,
}

impl ProfileRepository {
    /// Load every built-in, then merge custom profiles from `agent_dir`.
    /// A malformed custom profile is skipped with a warning, matching the
    /// previous listing behaviour. A missing/non-directory `agent_dir` is
    /// not an error — only the built-ins are returned.
    async fn load(agent_dir: Option<&Path>) -> Result<Self, AgentError> {
        let mut profiles = builtin::all();

        if let Some(dir) = agent_dir
            && dir.is_dir()
        {
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
                        Ok(agent) => upsert(&mut profiles, agent),
                        Err(e) => eprintln!("Warning: skipping {}: {e}", path.display()),
                    }
                }
            }
        }

        Ok(Self { profiles })
    }

    /// Look up a profile by exact name (built-in or custom override),
    /// including internal ones.
    fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.profiles.iter().find(|a| a.profile.name == name)
    }

    /// Profiles a user may list or select — excludes internal profiles.
    fn selectable(&self) -> impl Iterator<Item = &AgentDefinition> {
        self.profiles.iter().filter(|a| !a.profile.internal)
    }

    /// Comma-separated selectable names, for "did you mean" error text.
    fn selectable_names(&self) -> String {
        self.selectable()
            .map(|a| a.profile.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Resolve a single `--profile` value: a registry name first, then a
    /// filesystem path. Internal profiles are rejected as not selectable.
    async fn resolve(&self, value: &str) -> Result<AgentDefinition, AgentError> {
        // 1. Registry name — custom overrides already merged in `load`.
        if let Some(agent) = self.get(value) {
            if agent.profile.internal {
                return Err(AgentError::NotFound(format!(
                    "'{value}' is an internal profile and cannot be selected directly. \
                     Available profiles: {}",
                    self.selectable_names()
                )));
            }
            return Ok(agent.clone());
        }

        // 2. Direct file path.
        if value.contains('/') || value.ends_with(".md") {
            let path = Path::new(value);
            if path.exists() {
                let content =
                    tokio::fs::read_to_string(path)
                        .await
                        .map_err(|e| AgentError::ReadError {
                            path: value.to_string(),
                            source: e,
                        })?;
                return parser::parse_agent_definition(&content)
                    .map_err(|e| AgentError::ParseError(e.to_string()));
            }
            return Err(AgentError::NotFound(format!("file not found: {value}")));
        }

        // 3. Unknown.
        Err(AgentError::NotFound(format!(
            "unknown profile '{value}'. Available built-in profiles: {}",
            self.selectable_names()
        )))
    }
}

/// Insert `def` into `profiles`, replacing an existing entry of the same
/// name (a custom profile overrides the built-in) or appending it.
fn upsert(profiles: &mut Vec<AgentDefinition>, def: AgentDefinition) {
    if let Some(slot) = profiles
        .iter_mut()
        .find(|a| a.profile.name == def.profile.name)
    {
        *slot = def;
    } else {
        profiles.push(def);
    }
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
        assert!(
            !err.contains("critic") && !err.contains("triage"),
            "suggestions must not leak internal profiles, got: {err}"
        );
    }

    #[tokio::test]
    async fn resolve_internal_profiles_rejected() {
        for name in ["critic", "triage"] {
            let err = resolve_profiles(&[name.to_string()], None)
                .await
                .expect_err("internal profile must not be selectable")
                .to_string();
            assert!(err.contains("internal"), "got: {err}");
            assert!(
                err.contains(name),
                "error should name the rejected profile, got: {err}"
            );
        }
    }

    #[tokio::test]
    async fn list_all_excludes_internal_profiles() {
        let agents = list_all_profiles(None).await.unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(!names.contains(&"critic"), "got: {names:?}");
        assert!(!names.contains(&"triage"), "got: {names:?}");
    }

    #[tokio::test]
    async fn resolve_by_tag_excludes_internal_profiles() {
        // `triage` carries the `internal` tag, but being an internal
        // profile it must never surface through tag selection.
        let agents = resolve_profiles_by_tags(&["internal".to_string()], None)
            .await
            .unwrap();
        assert!(
            agents.is_empty(),
            "internal-tagged internal profile leaked: {:?}",
            agents.iter().map(|a| &a.profile.name).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn custom_override_of_internal_name_is_user_selectable() {
        // A user who ships their own `critic.md` (without `internal: true`)
        // deliberately reclaims that name as a regular reviewer — while the
        // built-in internal critic is still what the verify pass loads.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("critic.md"),
            "---\nname: critic\ndescription: My own critic\ntags: []\n---\nReview prompt.",
        )
        .unwrap();

        let agents = resolve_profiles(&["critic".to_string()], Some(dir.path()))
            .await
            .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].profile.description, "My own critic");
        assert!(!agents[0].profile.internal, "override is not internal");

        // The built-in critic the verify pass uses is unaffected.
        assert!(builtin::get_builtin("critic").unwrap().profile.internal);
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
        assert!(names.contains(&"general"));
        assert_eq!(agents.len(), 5);
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
        assert_eq!(agents.len(), 6);
    }

    #[tokio::test]
    async fn list_all_skips_invalid_custom_profiles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.md"), "no frontmatter").unwrap();

        let agents = list_all_profiles(Some(dir.path())).await.unwrap();
        // Only built-ins, bad.md skipped with warning
        assert_eq!(agents.len(), 5);
    }

    #[tokio::test]
    async fn list_all_nonexistent_agent_dir() {
        let result =
            list_all_profiles(Some(std::path::Path::new("/tmp/nitpik_no_such_dir_xyz"))).await;
        // Non-existent dir is not an error — it's just not a directory, so skip
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 5); // just built-ins
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
        assert!(names.contains(&"general"));
        assert_eq!(agents.len(), 5);
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

    // -----------------------------------------------------------------------
    // list_always_include_profiles
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn always_include_returns_builtin_security() {
        // The built-in `security` profile ships with `always_include: true`.
        let agents = list_always_include_profiles(None).await.unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(
            names.contains(&"security"),
            "built-in security should opt in; got: {names:?}"
        );
    }

    #[tokio::test]
    async fn always_include_other_builtins_do_not_opt_in() {
        // backend/frontend/architect should NOT be always-on by default.
        let agents = list_always_include_profiles(None).await.unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(!names.contains(&"backend"));
        assert!(!names.contains(&"frontend"));
        assert!(!names.contains(&"architect"));
    }

    #[tokio::test]
    async fn always_include_picks_up_custom_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("docs-drift.md"),
            "---\nname: docs-drift\ndescription: Flags doc drift\ntags: [docs]\nalways_include: true\n---\nReview prompt.",
        )
        .unwrap();
        // A regular (non-always) custom profile should NOT be returned.
        std::fs::write(
            dir.path().join("style.md"),
            "---\nname: style\ndescription: Style\ntags: []\n---\nPrompt.",
        )
        .unwrap();

        let agents = list_always_include_profiles(Some(dir.path()))
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(names.contains(&"docs-drift"), "got: {names:?}");
        assert!(
            names.contains(&"security"),
            "still includes built-in security; got: {names:?}"
        );
        assert!(
            !names.contains(&"style"),
            "non-always profile excluded; got: {names:?}"
        );
    }

    #[tokio::test]
    async fn always_include_user_override_can_disable_builtin_security() {
        // User ships a security.md override that opts OUT of always_include.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("security.md"),
            "---\nname: security\ndescription: Custom security\ntags: []\nalways_include: false\n---\nPrompt.",
        )
        .unwrap();

        let agents = list_always_include_profiles(Some(dir.path()))
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(
            !names.contains(&"security"),
            "user override disabled built-in always-on; got: {names:?}"
        );
    }

    #[tokio::test]
    async fn always_include_defaults_to_false_when_field_omitted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plain.md"),
            "---\nname: plain\ndescription: No flag\ntags: []\n---\nPrompt.",
        )
        .unwrap();

        let agents = list_always_include_profiles(Some(dir.path()))
            .await
            .unwrap();
        let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
        assert!(!names.contains(&"plain"), "got: {names:?}");
    }
}
