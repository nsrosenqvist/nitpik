//! Environment variable abstraction for testability.
//!
//! Production code uses [`Env::real()`] which delegates to [`std::env::var`].
//! Tests use [`Env::mock()`] backed by a `HashMap`, eliminating the need for
//! `unsafe` calls to [`std::env::set_var`] / [`std::env::remove_var`].

use std::collections::HashMap;

/// Environment variable reader.
///
/// Wraps lookups so that production code hits `std::env` while tests
/// can supply a controlled set of values.
#[derive(Clone, Debug)]
pub struct Env {
    overrides: Option<HashMap<String, String>>,
}

impl Env {
    /// Create an `Env` that reads from the real process environment.
    pub fn real() -> Self {
        Self { overrides: None }
    }

    /// Create an `Env` backed by explicit key-value pairs.
    #[cfg(test)]
    pub fn mock(vars: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>) -> Self {
        Self {
            overrides: Some(
                vars.into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            ),
        }
    }

    /// Look up an environment variable by name.
    pub fn var(&self, name: &str) -> Result<String, std::env::VarError> {
        match &self.overrides {
            Some(map) => map.get(name).cloned().ok_or(std::env::VarError::NotPresent),
            None => std::env::var(name),
        }
    }

    /// Returns `true` if the variable is present (non-empty).
    pub fn is_set(&self, name: &str) -> bool {
        self.var(name).is_ok()
    }
}

impl Default for Env {
    fn default() -> Self {
        Self::real()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_env_reads_cargo_manifest_dir() {
        let env = Env::real();
        assert!(env.var("CARGO_MANIFEST_DIR").is_ok());
    }

    #[test]
    fn mock_env_returns_set_values() {
        let env = Env::mock([("FOO", "bar"), ("BAZ", "qux")]);
        assert_eq!(env.var("FOO").unwrap(), "bar");
        assert_eq!(env.var("BAZ").unwrap(), "qux");
    }

    #[test]
    fn mock_env_returns_not_present_for_missing() {
        let env = Env::mock(Vec::<(&str, &str)>::new());
        assert!(env.var("NONEXISTENT").is_err());
    }

    #[test]
    fn is_set_checks_presence() {
        let env = Env::mock([("PRESENT", "value")]);
        assert!(env.is_set("PRESENT"));
        assert!(!env.is_set("ABSENT"));
    }
}
