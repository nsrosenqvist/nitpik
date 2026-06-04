//! Built-in agent profile registry.
//!
//! Profiles are embedded via `include_str!` so they ship with the binary.

use crate::agents::parser;
use crate::models::AgentDefinition;

const BACKEND_MD: &str = include_str!("backend.md");
const FRONTEND_MD: &str = include_str!("frontend.md");
const ARCHITECT_MD: &str = include_str!("architect.md");
const SECURITY_MD: &str = include_str!("security.md");
const GENERAL_MD: &str = include_str!("general.md");
const CRITIC_MD: &str = include_str!("critic.md");
const TRIAGE_MD: &str = include_str!("triage.md");

/// One built-in profile: its canonical name and embedded markdown body.
struct Builtin {
    name: &'static str,
    body: &'static str,
}

/// Every built-in profile, in display order. Adding a profile means
/// adding a single row here — name and body stay in lockstep, so there
/// is no separate name list to keep in sync. Whether a profile is
/// internal (`critic`, `triage`) is declared in its own frontmatter via
/// `internal: true`, not duplicated here.
const BUILTINS: &[Builtin] = &[
    Builtin { name: "backend", body: BACKEND_MD },
    Builtin { name: "frontend", body: FRONTEND_MD },
    Builtin { name: "architect", body: ARCHITECT_MD },
    Builtin { name: "security", body: SECURITY_MD },
    Builtin { name: "general", body: GENERAL_MD },
    Builtin { name: "critic", body: CRITIC_MD },
    Builtin { name: "triage", body: TRIAGE_MD },
];

/// Get a built-in agent definition by name.
///
/// Returns internal profiles (`critic`, `triage`) as well — this is the
/// single by-name accessor nitpik's own passes (verify, triage) use to
/// load those profiles deliberately. Callers resolving a *user* request
/// must check [`AgentProfile::internal`](crate::models::agent::AgentProfile::internal);
/// the profile-resolution layer in [`crate::agents`] does this for them.
pub fn get_builtin(name: &str) -> Option<AgentDefinition> {
    let body = BUILTINS.iter().find(|b| b.name == name)?.body;
    parser::parse_agent_definition(body).ok()
}

/// Parse and return every built-in profile, in declared order.
///
/// Panics if an embedded profile fails to parse — that is a build-time
/// authoring error in this crate, not a runtime condition. The
/// `all_builtins_parse` test guards against shipping one.
pub fn all() -> Vec<AgentDefinition> {
    BUILTINS
        .iter()
        .map(|b| {
            parser::parse_agent_definition(b.body)
                .unwrap_or_else(|e| panic!("built-in profile '{}' failed to parse: {e}", b.name))
        })
        .collect()
}

/// List all built-in profile names (including internal ones).
pub fn list_builtin_names() -> Vec<&'static str> {
    BUILTINS.iter().map(|b| b.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtins_parse() {
        for name in list_builtin_names() {
            let agent = get_builtin(name)
                .unwrap_or_else(|| panic!("built-in profile '{name}' failed to parse"));
            assert_eq!(agent.profile.name, name);
            assert!(!agent.system_prompt.is_empty());
        }
        // `all()` parses the same set (and would panic on a bad profile).
        assert_eq!(all().len(), list_builtin_names().len());
    }

    #[test]
    fn unknown_builtin_returns_none() {
        assert!(get_builtin("nonexistent").is_none());
    }

    #[test]
    fn list_names() {
        let names = list_builtin_names();
        assert!(names.contains(&"backend"));
        assert!(names.contains(&"security"));
    }

    #[test]
    fn security_is_always_include() {
        let security = get_builtin("security").unwrap();
        assert!(
            security.profile.always_include,
            "built-in security must opt into always_include"
        );
    }

    #[test]
    fn other_builtins_are_not_always_include() {
        for name in ["backend", "frontend", "architect", "general"] {
            let agent = get_builtin(name).unwrap();
            assert!(
                !agent.profile.always_include,
                "{name} should not be always-on by default"
            );
        }
    }

    #[test]
    fn critic_profile_is_internal_not_a_reviewer() {
        let critic = get_builtin("critic").unwrap();
        assert_eq!(critic.profile.name, "critic");
        // Marked internal so the resolution layer keeps it out of
        // listings, tags, auto, and explicit `--profile`.
        assert!(critic.profile.internal);
        // Critic should not be auto-selected like a normal reviewer.
        assert!(!critic.profile.always_include);
        // Sanity: prompt mentions verdict format.
        assert!(critic.system_prompt.contains("keep"));
        assert!(critic.system_prompt.contains("drop"));
    }

    #[test]
    fn triage_profile_is_internal() {
        let triage = get_builtin("triage").unwrap();
        assert!(triage.profile.internal, "triage must be internal");
    }

    #[test]
    fn reviewer_builtins_are_not_internal() {
        for name in ["backend", "frontend", "architect", "security", "general"] {
            assert!(
                !get_builtin(name).unwrap().profile.internal,
                "{name} must remain user-selectable"
            );
        }
    }
}
