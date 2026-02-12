//! App-wide constants.
//!
//! Centralises the tool name, config paths, environment variable names,
//! and URLs so a rename only requires changing this file.

/// Display name of the tool (lowercase).
pub const APP_NAME: &str = "nitpik";

/// Local config filename (e.g. `.nitpik.toml` in repo root).
pub const CONFIG_FILENAME: &str = ".nitpik.toml";

/// Directory name under `~/.config/` for global config and cache.
pub const CONFIG_DIR: &str = "nitpik";

/// Telemetry heartbeat endpoint.
pub const TELEMETRY_URL: &str = "https://nitpik.dev/v1/heartbeat";


// ── Environment variable names ──────────────────────────────────────

pub const ENV_PROVIDER: &str = "NITPIK_PROVIDER";
pub const ENV_MODEL: &str = "NITPIK_MODEL";
pub const ENV_API_KEY: &str = "NITPIK_API_KEY";
pub const ENV_BASE_URL: &str = "NITPIK_BASE_URL";
pub const ENV_LICENSE_KEY: &str = "NITPIK_LICENSE_KEY";
pub const ENV_TELEMETRY: &str = "NITPIK_TELEMETRY";
