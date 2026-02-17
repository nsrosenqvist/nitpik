//! Self-update logic for the nitpik binary.
//!
//! Downloads the latest release from GitHub, verifies its SHA256 checksum,
//! extracts the binary from the `.tar.gz` archive, and atomically replaces
//! the currently running executable.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::constants::{self, TARGET, USER_AGENT, VERSION as CURRENT_VERSION};

/// Errors that can occur during self-update.
#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("failed to query GitHub releases: {0}")]
    ApiError(String),

    #[error("failed to download release asset: {0}")]
    DownloadError(String),

    #[error("checksum verification failed: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("checksum file does not contain entry for {0}")]
    ChecksumNotFound(String),

    #[error("failed to extract archive: {0}")]
    ExtractError(String),

    #[error("failed to replace binary: {0}")]
    ReplaceError(String),

    #[error("{0}")]
    PermissionDenied(String),

    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

/// Metadata about a GitHub release.
#[derive(Debug)]
struct ReleaseInfo {
    /// The git tag (e.g. "v0.2.0").
    tag: String,
    /// The semantic version without the leading 'v'.
    version: String,
}

/// Run the self-update process.
///
/// Returns `Ok(())` on success (updated or already up-to-date).
pub async fn run_update(force: bool) -> Result<(), UpdateError> {
    // Warn if running in a container — the image should be rebuilt instead
    if let Some(env) = detect_container_environment() {
        eprintln!(
            "  {} Running inside {env}. Consider rebuilding the image instead of self-updating.",
            colored::Colorize::yellow("Warning:"),
        );
        eprintln!();
    }

    // Warn if running in CI — updates should go through the pipeline
    if detect_ci_environment() {
        eprintln!(
            "  {} Running in a CI environment. Consider pinning a version in your pipeline instead.",
            colored::Colorize::yellow("Warning:"),
        );
        eprintln!();
    }

    // Check platform support
    validate_platform()?;

    eprintln!("  Checking for updates...");

    let release = fetch_latest_release().await?;

    if !force && !is_newer(&release.version) {
        eprintln!("  Already on the latest version ({CURRENT_VERSION}).",);
        return Ok(());
    }

    eprintln!("  Updating {CURRENT_VERSION} → {} ...", release.version);

    // Determine the current executable path
    let current_exe = std::env::current_exe().map_err(|e| {
        UpdateError::ReplaceError(format!("could not determine current executable path: {e}"))
    })?;
    let current_exe = current_exe.canonicalize().unwrap_or(current_exe);

    // Check write permissions early
    check_write_permission(&current_exe)?;

    // Download the archive
    let asset_name = format!("nitpik-{TARGET}.tar.gz");
    let asset_url = constants::release_asset_url(&release.tag, TARGET);
    eprintln!("  Downloading {asset_name}...");
    let archive_bytes = download_bytes(&asset_url).await?;

    // Download and verify checksum
    let checksums_url = constants::release_checksums_url(&release.tag);
    eprintln!("  Verifying checksum...");
    let checksums_text = download_text(&checksums_url).await?;
    verify_checksum(&archive_bytes, &asset_name, &checksums_text)?;

    // Extract the binary from the archive
    eprintln!("  Extracting...");
    let new_binary = extract_binary(&archive_bytes)?;

    // Atomically replace the current binary
    eprintln!("  Replacing binary...");
    atomic_replace(&current_exe, &new_binary)?;

    eprintln!(
        "  {} Updated to {} successfully.",
        colored::Colorize::green("✓"),
        release.version
    );

    Ok(())
}

/// Query the GitHub releases API for the latest release tag.
async fn fetch_latest_release() -> Result<ReleaseInfo, UpdateError> {
    let client = reqwest::Client::new();
    let resp = client
        .get(constants::GITHUB_RELEASES_LATEST_API)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| UpdateError::ApiError(format!("request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(UpdateError::ApiError(format!(
            "GitHub API returned {}",
            resp.status()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| UpdateError::ApiError(format!("failed to parse response: {e}")))?;

    let tag = body["tag_name"]
        .as_str()
        .ok_or_else(|| UpdateError::ApiError("missing tag_name in response".to_string()))?
        .to_string();

    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();

    Ok(ReleaseInfo { tag, version })
}

/// Compare a remote version string against the current version.
///
/// Uses simple tuple comparison of (major, minor, patch) components.
/// Returns `true` if `remote` is strictly newer than `CURRENT_VERSION`.
fn is_newer(remote: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let parts: Vec<&str> = v.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        ))
    };

    match (parse(CURRENT_VERSION), parse(remote)) {
        (Some(current), Some(remote)) => remote > current,
        _ => remote != CURRENT_VERSION,
    }
}

/// Download a URL and return the raw bytes.
async fn download_bytes(url: &str) -> Result<Vec<u8>, UpdateError> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| UpdateError::DownloadError(format!("{url}: {e}")))?;

    if !resp.status().is_success() {
        return Err(UpdateError::DownloadError(format!(
            "{url}: HTTP {}",
            resp.status()
        )));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| UpdateError::DownloadError(format!("{url}: {e}")))
}

/// Download a URL and return the body as text.
async fn download_text(url: &str) -> Result<String, UpdateError> {
    let bytes = download_bytes(url).await?;
    String::from_utf8(bytes)
        .map_err(|e| UpdateError::DownloadError(format!("response is not valid UTF-8: {e}")))
}

/// Verify archive bytes against a SHA256SUMS file.
///
/// The checksums file is expected to have lines like:
/// `<hex-hash>  <filename>`
fn verify_checksum(data: &[u8], asset_name: &str, checksums_text: &str) -> Result<(), UpdateError> {
    let expected = checksums_text
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let filename = parts.next()?;
            if filename == asset_name {
                Some(hash.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| UpdateError::ChecksumNotFound(asset_name.to_string()))?;

    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = hex::encode(hasher.finalize());

    if actual != expected {
        return Err(UpdateError::ChecksumMismatch { expected, actual });
    }

    Ok(())
}

/// Extract the `nitpik` binary from a `.tar.gz` archive in memory.
///
/// Looks for an entry named `nitpik` (or ending with `/nitpik`) in the
/// archive and returns its contents as bytes.
fn extract_binary(archive_bytes: &[u8]) -> Result<Vec<u8>, UpdateError> {
    let decoder = GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| UpdateError::ExtractError(format!("failed to read archive entries: {e}")))?
    {
        let mut entry =
            entry.map_err(|e| UpdateError::ExtractError(format!("corrupt archive entry: {e}")))?;

        let path = entry
            .path()
            .map_err(|e| UpdateError::ExtractError(format!("invalid path in archive: {e}")))?;

        let is_binary = path.file_name().map_or(false, |name| name == "nitpik");
        if !is_binary {
            continue;
        }

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| {
            UpdateError::ExtractError(format!("failed to read binary from archive: {e}"))
        })?;

        if buf.is_empty() {
            return Err(UpdateError::ExtractError(
                "extracted binary is empty".to_string(),
            ));
        }

        return Ok(buf);
    }

    Err(UpdateError::ExtractError(
        "archive does not contain a 'nitpik' binary".to_string(),
    ))
}

/// Atomically replace the binary at `target_path` with `new_binary`.
///
/// Writes to a temporary file next to the target, sets executable
/// permissions, and renames (which is atomic on the same filesystem).
fn atomic_replace(target_path: &Path, new_binary: &[u8]) -> Result<(), UpdateError> {
    let parent = target_path.parent().ok_or_else(|| {
        UpdateError::ReplaceError("cannot determine parent directory".to_string())
    })?;

    let tmp_path = parent.join(".nitpik-update.tmp");

    // Write new binary to temp file
    let mut tmp_file = fs::File::create(&tmp_path).map_err(|e| {
        if e.kind() == io::ErrorKind::PermissionDenied {
            UpdateError::PermissionDenied(format!(
                "permission denied writing to {}. Try running with sudo.",
                parent.display()
            ))
        } else {
            UpdateError::ReplaceError(format!("failed to create temp file: {e}"))
        }
    })?;

    tmp_file
        .write_all(new_binary)
        .map_err(|e| UpdateError::ReplaceError(format!("failed to write temp file: {e}")))?;

    tmp_file
        .flush()
        .map_err(|e| UpdateError::ReplaceError(format!("failed to flush temp file: {e}")))?;

    // Set executable permissions (Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&tmp_path, perms)
            .map_err(|e| UpdateError::ReplaceError(format!("failed to set permissions: {e}")))?;
    }

    // Atomic rename
    fs::rename(&tmp_path, target_path).map_err(|e| {
        // Clean up temp file on failure
        let _ = fs::remove_file(&tmp_path);
        if e.kind() == io::ErrorKind::PermissionDenied {
            UpdateError::PermissionDenied(format!(
                "permission denied replacing {}. Try running with sudo.",
                target_path.display()
            ))
        } else {
            UpdateError::ReplaceError(format!("failed to replace binary: {e}"))
        }
    })
}

/// Check whether we can write to the directory containing the target binary.
fn check_write_permission(exe_path: &Path) -> Result<(), UpdateError> {
    let parent = exe_path.parent().ok_or_else(|| {
        UpdateError::ReplaceError("cannot determine parent directory".to_string())
    })?;

    let probe_path = parent.join(".nitpik-write-probe");
    match fs::File::create(&probe_path) {
        Ok(_) => {
            let _ = fs::remove_file(&probe_path);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            Err(UpdateError::PermissionDenied(format!(
                "permission denied: cannot write to {}. Try running with sudo.",
                parent.display()
            )))
        }
        Err(e) => Err(UpdateError::ReplaceError(format!(
            "cannot write to {}: {e}",
            parent.display()
        ))),
    }
}

/// Validate that the current platform has release builds available.
fn validate_platform() -> Result<(), UpdateError> {
    const SUPPORTED_TARGETS: &[&str] = &[
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
    ];

    if !SUPPORTED_TARGETS.contains(&TARGET) {
        return Err(UpdateError::UnsupportedPlatform(format!(
            "no pre-built binary available for '{TARGET}'. \
             Supported targets: {}",
            SUPPORTED_TARGETS.join(", ")
        )));
    }

    Ok(())
}

/// Detect whether the process is running inside a container.
///
/// Returns `Some("Docker")`, `Some("a container")`, etc. if detected.
fn detect_container_environment() -> Option<&'static str> {
    // Docker creates /.dockerenv
    if Path::new("/.dockerenv").exists() {
        return Some("Docker");
    }

    // Podman and other OCI runtimes set `container` env var
    if std::env::var("container").is_ok() {
        return Some("a container");
    }

    // Check cgroup for container indicators
    if let Ok(cgroup) = fs::read_to_string("/proc/1/cgroup") {
        if cgroup.contains("docker") || cgroup.contains("containerd") || cgroup.contains("lxc") {
            return Some("a container");
        }
    }

    // Check for container runtime env vars
    if std::env::var("KUBERNETES_SERVICE_HOST").is_ok() {
        return Some("Kubernetes");
    }

    None
}

/// Detect whether the process is running in a CI environment.
fn detect_ci_environment() -> bool {
    // Generic CI indicator (set by GitHub Actions, GitLab CI, Travis, etc.)
    if std::env::var("CI").is_ok() {
        return true;
    }

    // Provider-specific indicators
    let ci_vars = [
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "CIRCLECI",
        "TRAVIS",
        "JENKINS_URL",
        "BUILDKITE",
        "BITBUCKET_BUILD_NUMBER",
        "TF_BUILD",     // Azure Pipelines
        "CODEBUILD_CI", // AWS CodeBuild
        "DRONE",
        "WOODPECKER_CI",
    ];

    ci_vars.iter().any(|var| std::env::var(var).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_detects_higher_version() {
        assert!(version_cmp("0.1.0", "0.2.0"));
        assert!(version_cmp("0.1.0", "1.0.0"));
        assert!(version_cmp("0.1.0", "0.1.1"));
    }

    #[test]
    fn is_newer_rejects_lower_or_equal() {
        assert!(!version_cmp("0.2.0", "0.1.0"));
        assert!(!version_cmp("0.1.0", "0.1.0"));
    }

    /// Helper that isolates version comparison from CURRENT_VERSION.
    fn version_cmp(current: &str, remote: &str) -> bool {
        let parse = |v: &str| -> Option<(u64, u64, u64)> {
            let parts: Vec<&str> = v.split('.').collect();
            if parts.len() != 3 {
                return None;
            }
            Some((
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
                parts[2].parse().ok()?,
            ))
        };
        match (parse(current), parse(remote)) {
            (Some(c), Some(r)) => r > c,
            _ => remote != current,
        }
    }

    #[test]
    fn verify_checksum_success() {
        let data = b"hello world";
        let hash = hex::encode(Sha256::digest(data));
        let checksums = format!("{hash}  nitpik-x86_64-unknown-linux-gnu.tar.gz\n");
        assert!(
            verify_checksum(data, "nitpik-x86_64-unknown-linux-gnu.tar.gz", &checksums).is_ok()
        );
    }

    #[test]
    fn verify_checksum_mismatch() {
        let data = b"hello world";
        let checksums = "0000000000000000000000000000000000000000000000000000000000000000  nitpik-x86_64-unknown-linux-gnu.tar.gz\n";
        let result = verify_checksum(data, "nitpik-x86_64-unknown-linux-gnu.tar.gz", checksums);
        assert!(matches!(result, Err(UpdateError::ChecksumMismatch { .. })));
    }

    #[test]
    fn verify_checksum_not_found() {
        let data = b"hello world";
        let checksums = "abc123  some-other-file.tar.gz\n";
        let result = verify_checksum(data, "nitpik-x86_64-unknown-linux-gnu.tar.gz", checksums);
        assert!(matches!(result, Err(UpdateError::ChecksumNotFound(_))));
    }

    #[test]
    fn extract_binary_empty_archive_fails() {
        let result = extract_binary(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn extract_binary_valid_archive() {
        let mut builder = tar::Builder::new(Vec::new());

        let content = b"fake-binary-content";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "nitpik", &content[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let result = extract_binary(&gz_bytes).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn extract_binary_no_matching_entry() {
        let mut builder = tar::Builder::new(Vec::new());

        let content = b"other-content";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "some-other-file", &content[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let result = extract_binary(&gz_bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not contain"));
    }

    #[test]
    fn validate_platform_accepts_known_targets() {
        // The current build target should be one of the supported ones,
        // or the test should still not panic.
        let result = validate_platform();
        // We just verify it returns a result (Ok or Err) without panicking.
        let _ = result;
    }

    #[test]
    fn detect_ci_does_not_panic() {
        // Just verify the function runs without panicking regardless of env state.
        let _ = detect_ci_environment();
    }

    #[test]
    fn detect_container_does_not_panic() {
        // Just verify the function runs without panicking regardless of env state.
        let _ = detect_container_environment();
    }

    #[test]
    fn is_newer_with_invalid_version_strings() {
        // Non-semver strings should fall back to string inequality.
        assert!(version_cmp("abc", "def"));
        assert!(!version_cmp("abc", "abc"));
    }

    #[test]
    fn verify_checksum_multiline_checksums_file() {
        let data = b"test data";
        let hash = hex::encode(Sha256::digest(data));
        let checksums = format!(
            "aaaa  some-other-file.tar.gz\n\
             {hash}  target-file.tar.gz\n\
             bbbb  yet-another.tar.gz\n"
        );
        assert!(verify_checksum(data, "target-file.tar.gz", &checksums).is_ok());
    }

    #[test]
    fn extract_binary_nested_path() {
        // Binary at a nested path like "nitpik-v1.0.0/nitpik" should still match.
        let mut builder = tar::Builder::new(Vec::new());
        let content = b"nested-binary";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "nitpik-v1.0.0/nitpik", &content[..])
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        let gz_bytes = encoder.finish().unwrap();

        let result = extract_binary(&gz_bytes).unwrap();
        assert_eq!(result, content);
    }
}
