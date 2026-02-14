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
