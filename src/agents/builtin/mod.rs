//! Built-in agent profile registry.
//!
//! Profiles are embedded via `include_str!` so they ship with the binary.

use crate::agents::parser;
use crate::models::AgentDefinition;

// Issue-typed lenses — the default review engine (see
// plans/pr-native-review/lens-model.md). `security` and `correctness` are
// always-on; the rest are triage-selected.
const SECURITY_MD: &str = include_str!("security.md");
const CORRECTNESS_MD: &str = include_str!("correctness.md");
const CONCURRENCY_MD: &str = include_str!("concurrency.md");
const PERFORMANCE_MD: &str = include_str!("performance.md");
const TEST_INTEGRITY_MD: &str = include_str!("test-integrity.md");
const OPERATIONAL_MD: &str = include_str!("operational.md");
const A11Y_MD: &str = include_str!("a11y.md");
const USER_JOURNEY_MD: &str = include_str!("user-journey.md");
const CONTRACT_IMPACT_MD: &str = include_str!("contract-impact.md");
const DOCS_DRIFT_MD: &str = include_str!("docs-drift.md");
const HOLISTIC_MD: &str = include_str!("holistic.md");
const MALICIOUS_MD: &str = include_str!("malicious.md");

// Internal profiles for nitpik's own passes.
const CRITIC_MD: &str = include_str!("critic.md");
const CRITIC_SOUNDNESS_MD: &str = include_str!("critic-soundness.md");
const CRITIC_GROUNDING_MD: &str = include_str!("critic-grounding.md");
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
    // Lenses (default engine).
    Builtin {
        name: "security",
        body: SECURITY_MD,
    },
    Builtin {
        name: "correctness",
        body: CORRECTNESS_MD,
    },
    Builtin {
        name: "concurrency",
        body: CONCURRENCY_MD,
    },
    Builtin {
        name: "performance",
        body: PERFORMANCE_MD,
    },
    Builtin {
        name: "test-integrity",
        body: TEST_INTEGRITY_MD,
    },
    Builtin {
        name: "operational",
        body: OPERATIONAL_MD,
    },
    Builtin {
        name: "a11y",
        body: A11Y_MD,
    },
    Builtin {
        name: "user-journey",
        body: USER_JOURNEY_MD,
    },
    Builtin {
        name: "contract-impact",
        body: CONTRACT_IMPACT_MD,
    },
    Builtin {
        name: "docs-drift",
        body: DOCS_DRIFT_MD,
    },
    Builtin {
        name: "holistic",
        body: HOLISTIC_MD,
    },
    // Malice-hunting lens — opt-in (named via `--profile malicious`/`--tag`),
    // never auto-on. Whole-diff scope so it can connect cross-file intent.
    Builtin {
        name: "malicious",
        body: MALICIOUS_MD,
    },
    // Internal passes.
    Builtin {
        name: "critic",
        body: CRITIC_MD,
    },
    Builtin {
        name: "critic-soundness",
        body: CRITIC_SOUNDNESS_MD,
    },
    Builtin {
        name: "critic-grounding",
        body: CRITIC_GROUNDING_MD,
    },
    Builtin {
        name: "triage",
        body: TRIAGE_MD,
    },
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
        assert!(names.contains(&"correctness"));
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
    fn correctness_is_always_include() {
        let correctness = get_builtin("correctness").unwrap();
        assert!(
            correctness.profile.always_include,
            "built-in correctness must opt into always_include"
        );
    }

    #[test]
    fn other_builtins_are_not_always_include() {
        // The always-on floor is exactly security + correctness; every other
        // lens and legacy profile is conditional / opt-in.
        for name in [
            "concurrency",
            "performance",
            "test-integrity",
            "operational",
            "a11y",
            "user-journey",
            "contract-impact",
            "docs-drift",
            "holistic",
        ] {
            let agent = get_builtin(name).unwrap();
            assert!(
                !agent.profile.always_include,
                "{name} should not be always-on by default"
            );
        }
    }

    #[test]
    fn whole_diff_lenses_declare_scope_and_agentic() {
        for name in ["contract-impact", "docs-drift", "holistic"] {
            let agent = get_builtin(name).unwrap();
            assert_eq!(
                agent.profile.scope,
                crate::models::LensScope::Diff,
                "{name} must be a whole-diff lens"
            );
            assert!(agent.profile.agentic, "{name} must be agentic");
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
        for name in ["security", "correctness", "a11y", "holistic"] {
            assert!(
                !get_builtin(name).unwrap().profile.internal,
                "{name} must remain user-selectable"
            );
        }
    }
}
