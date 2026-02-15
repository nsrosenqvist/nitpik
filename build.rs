/// Expose build metadata as compile-time environment variables.
///
/// Emits:
/// - `TARGET` — the compilation target triple (e.g. `x86_64-unknown-linux-gnu`)
/// - `GIT_SHA` — short git commit hash, or `"unknown"` outside a git repo
/// - `BUILD_DATE` — build date as `YYYY-MM-DD`, or `"unknown"` on failure
fn main() {
    // Target triple (used by the self-update module)
    println!(
        "cargo:rustc-env=TARGET={}",
        std::env::var("TARGET").unwrap()
    );

    // Git commit hash
    let git_sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_SHA={git_sha}");

    // Full version string.
    //
    // Release builds (HEAD tagged `v{CARGO_PKG_VERSION}`): clean semver,
    //   e.g. "0.5.0"
    //
    // Dev builds: derived from `git describe --tags --match 'v*'` which
    //   gives e.g. "v0.5.0-12-g0208df1" (12 commits after v0.5.0).
    //   We reformat to semver pre-release: "0.5.0-dev.12+0208df1".
    //   If there are no tags yet, falls back to "0.0.0-dev+{sha}".
    let cargo_version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let is_release = std::process::Command::new("git")
        .args(["tag", "--points-at", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|tags| {
            tags.lines()
                .any(|t| t.trim() == format!("v{cargo_version}"))
        })
        .unwrap_or(false);

    let full_version = if is_release || git_sha == "unknown" {
        cargo_version
    } else {
        // Try `git describe` to get the distance from the last version tag.
        let described = std::process::Command::new("git")
            .args(["describe", "--tags", "--match", "v*", "--long"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string());

        match described {
            // Format: "v0.5.0-12-g0208df1" → "0.5.0-dev.12+0208df1"
            Some(desc) => parse_git_describe(&desc, &git_sha),
            // No version tags exist yet
            None => format!("0.0.0-dev+{git_sha}"),
        }
    };
    println!("cargo:rustc-env=FULL_VERSION={full_version}");

    // Build date (UTC)
    let build_date = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_DATE={build_date}");

    // Rebuild when the git HEAD changes (new commit or checkout)
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}

/// Parse `git describe --long` output into a semver-compatible dev version.
///
/// Input format:  `v0.5.0-12-g0208df1`  (tag-distance-gSHA)
/// Output format: `0.5.0-dev.12+0208df1`
///
/// If `distance` is 0 the tag points at HEAD (release), so return
/// the clean version. Falls back to `0.0.0-dev+{sha}` on parse failure.
fn parse_git_describe(desc: &str, sha: &str) -> String {
    // Split from the right: the last segment is `g<sha>`, the one before is
    // the commit distance, and everything before that is the tag name.
    let parts: Vec<&str> = desc.rsplitn(3, '-').collect();
    if parts.len() == 3 {
        let tag = parts[2].strip_prefix('v').unwrap_or(parts[2]);
        let distance = parts[1];
        if distance == "0" {
            // Exactly on the tag — clean version
            tag.to_string()
        } else {
            format!("{tag}-dev.{distance}+{sha}")
        }
    } else {
        // Unexpected format — safe fallback
        format!("0.0.0-dev+{sha}")
    }
}
