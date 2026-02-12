//! Built-in agent profile registry.
//!
//! Profiles are embedded via `include_str!` so they ship with the binary.

use crate::agents::parser;
use crate::models::AgentDefinition;

const BACKEND_MD: &str = include_str!("backend.md");
const FRONTEND_MD: &str = include_str!("frontend.md");
const ARCHITECT_MD: &str = include_str!("architect.md");
const SECURITY_MD: &str = include_str!("security.md");

/// List of all built-in profile names.
const BUILTIN_NAMES: &[&str] = &["backend", "frontend", "architect", "security"];

/// Get a built-in agent definition by name.
pub fn get_builtin(name: &str) -> Option<AgentDefinition> {
    let md = match name {
        "backend" => BACKEND_MD,
        "frontend" => FRONTEND_MD,
        "architect" => ARCHITECT_MD,
        "security" => SECURITY_MD,
        _ => return None,
    };

    parser::parse_agent_definition(md).ok()
}

/// List all available built-in profile names.
pub fn list_builtin_names() -> Vec<&'static str> {
    BUILTIN_NAMES.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtins_parse() {
        for name in BUILTIN_NAMES {
            let agent = get_builtin(name)
                .unwrap_or_else(|| panic!("built-in profile '{name}' failed to parse"));
            assert_eq!(agent.profile.name, *name);
            assert!(!agent.system_prompt.is_empty());
        }
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
}
