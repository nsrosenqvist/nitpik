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


// ── GitHub releases (self-update) ───────────────────────────────────

/// GitHub owner/repo for release downloads.
pub const GITHUB_REPO: &str = "nsrosenqvist/nitpik";

/// GitHub API endpoint for the latest release metadata.
pub const GITHUB_RELEASES_LATEST_API: &str =
    "https://api.github.com/repos/nsrosenqvist/nitpik/releases/latest";

/// Build a download URL for a release asset.
pub fn release_asset_url(tag: &str, target: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/nitpik-{}.tar.gz",
        GITHUB_REPO, tag, target
    )
}

/// Build a download URL for the SHA256SUMS file of a release.
pub fn release_checksums_url(tag: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/SHA256SUMS",
        GITHUB_REPO, tag
    )
}

// ── Environment variable names ──────────────────────────────────────

pub const ENV_PROVIDER: &str = "NITPIK_PROVIDER";
pub const ENV_MODEL: &str = "NITPIK_MODEL";
pub const ENV_API_KEY: &str = "NITPIK_API_KEY";
pub const ENV_BASE_URL: &str = "NITPIK_BASE_URL";
pub const ENV_LICENSE_KEY: &str = "NITPIK_LICENSE_KEY";
pub const ENV_TELEMETRY: &str = "NITPIK_TELEMETRY";
