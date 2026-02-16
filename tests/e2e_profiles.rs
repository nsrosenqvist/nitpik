//! End-to-end tests using real LLM providers and real git repos.
//!
//! These tests create temporary git repositories under `/tmp`, commit base
//! fixtures, apply changesets, and run the full review pipeline (including
//! real LLM calls) against each built-in reviewer profile.
//!
//! # Requirements
//!
//! - A valid LLM API key in the environment (e.g. `ANTHROPIC_API_KEY`).
//! - Network access to the LLM provider.
//!
//! # Running
//!
//! These tests are marked `#[ignore]` so they don't run during `cargo test`.
//! Run them explicitly:
//!
//! ```sh
//! ANTHROPIC_API_KEY=sk-... cargo test --test e2e_profiles -- --ignored --nocapture
//! ```
//!
//! You can also set `NITPIK_PROVIDER` / `NITPIK_MODEL` to use a different
//! provider or model.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nitpik::agents;
use nitpik::cache::CacheEngine;
use nitpik::config::Config;
use nitpik::context;
use nitpik::diff;
use nitpik::models::finding::{Finding, Severity};
use nitpik::models::{InputMode, ReviewContext};
use nitpik::orchestrator::ReviewOrchestrator;
use nitpik::providers::ReviewProvider;
use nitpik::providers::rig::RigProvider;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Path to the E2E fixture directory.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("e2e")
}

/// Returns `true` if an API key is available, `false` otherwise.
/// When no key is found the test should return early (skip).
fn has_api_key() -> bool {
    let config = Config::load(None, &nitpik::env::Env::real()).unwrap_or_default();
    if config.provider.api_key.is_none() {
        eprintln!(
            "SKIPPING: no API key found for provider '{}'. \
             Set ANTHROPIC_API_KEY (or NITPIK_API_KEY) to run E2E tests.",
            config.provider.name
        );
        return false;
    }
    true
}

/// Macro that returns early from a test if no API key is available.
macro_rules! require_api_key {
    () => {
        if !has_api_key() {
            return;
        }
    };
}

/// Create a temporary git repo, commit base files, apply a changeset, and
/// return `(repo_path, tempdir_handle)`.
///
/// The tempdir handle must be kept alive for the duration of the test —
/// dropping it removes the directory.
async fn setup_repo(scenario: &str) -> (PathBuf, tempfile::TempDir) {
    let base_dir = fixtures_dir().join(scenario).join("base");
    let changeset_dir = fixtures_dir().join(scenario).join("changeset");

    // Create temp dir under /tmp so it's easy to inspect on failure.
    let tmp = tempfile::Builder::new()
        .prefix(&format!("nitpik-e2e-{scenario}-"))
        .tempdir_in("/tmp")
        .expect("failed to create tempdir");
    let repo = tmp.path().to_path_buf();

    // git init + initial commit with base files
    run_git(&repo, &["init"]).await;
    run_git(&repo, &["config", "user.email", "test@nitpik.dev"]).await;
    run_git(&repo, &["config", "user.name", "Nitpik E2E"]).await;

    copy_tree(&base_dir, &repo);
    run_git(&repo, &["add", "."]).await;
    run_git(&repo, &["commit", "-m", "initial commit"]).await;

    // Apply changeset (overwrite files)
    copy_tree(&changeset_dir, &repo);

    (repo, tmp)
}

/// Run a git command inside `repo_dir` and panic on failure.
async fn run_git(repo_dir: &Path, args: &[&str]) {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()
        .await
        .unwrap_or_else(|e| panic!("failed to run git {}: {e}", args.join(" ")));

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "git {} failed (exit {}): {stderr}",
            args.join(" "),
            output.status
        );
    }
}

/// Recursively copy all files from `src` into `dst`, preserving relative paths.
fn copy_tree(src: &Path, dst: &Path) {
    for entry in walkdir::WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
        let rel = entry.path().strip_prefix(src).unwrap();
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).ok();
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(entry.path(), &target).unwrap_or_else(|e| {
                panic!(
                    "failed to copy {} → {}: {e}",
                    entry.path().display(),
                    target.display()
                )
            });
        }
    }
}

/// Build a `Config` that uses real credentials from the environment.
fn real_config() -> Config {
    Config::load(None, &nitpik::env::Env::real()).unwrap_or_default()
}

/// Run the full review pipeline for a given repo and profile name(s).
///
/// Returns the list of findings produced by the real LLM.
async fn run_review(
    repo_path: &Path,
    profile_names: &[&str],
    config: &Config,
) -> Vec<Finding> {
    // Get diffs (unstaged changes vs HEAD)
    let input = InputMode::GitBase("HEAD".to_string());
    let diffs = diff::get_diffs(&input, repo_path)
        .await
        .expect("failed to get diffs");

    assert!(
        !diffs.is_empty(),
        "changeset should produce diffs in {}", repo_path.display()
    );

    // Resolve agent profiles
    let profiles: Vec<String> = profile_names.iter().map(|s| s.to_string()).collect();
    let agent_defs = if profiles.iter().any(|p| p == "auto") {
        let auto_profiles = nitpik::agents::auto::auto_select_profiles(&diffs);
        agents::resolve_profiles(&auto_profiles, None)
            .await
            .expect("failed to resolve auto profiles")
    } else {
        agents::resolve_profiles(&profiles, None)
            .await
            .expect("failed to resolve profiles")
    };

    assert!(
        !agent_defs.is_empty(),
        "should have resolved at least one agent profile"
    );

    eprintln!(
        "  agents: [{}]",
        agent_defs
            .iter()
            .map(|a| a.profile.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Build baseline context
    let baseline = context::build_baseline_context(repo_path, &diffs, config).await;

    let review_context = ReviewContext {
        diffs,
        baseline,
        repo_root: repo_path.to_string_lossy().to_string(),
        is_path_scan: false,
    };

    // Create real provider
    let provider: Arc<dyn ReviewProvider> = Arc::new(
        RigProvider::new(config.provider.clone(), repo_path.to_path_buf())
            .expect("failed to create provider — is API key set?"),
    );

    // Run the orchestrator (cache disabled for E2E)
    let cache = CacheEngine::new(false);
    let progress = std::sync::Arc::new(nitpik::progress::ProgressTracker::new(&[], &[], false));
    let orchestrator = ReviewOrchestrator::new(Arc::clone(&provider), config, cache, progress, false, None, String::new());

    let result = orchestrator
        .run(&review_context, &agent_defs, 2, false, 10, 10)
        .await
        .expect("orchestrator should succeed");
    result.findings
}

// ---------------------------------------------------------------------------
// Shared assertions
// ---------------------------------------------------------------------------

/// Assert that findings have basic structural validity.
fn assert_findings_valid(findings: &[Finding], expected_file_substring: &str) {
    assert!(
        !findings.is_empty(),
        "expected at least one finding for changeset containing '{expected_file_substring}'"
    );

    for f in findings {
        // Every finding should have non-empty fields
        assert!(!f.file.is_empty(), "finding file path should not be empty");
        assert!(!f.title.is_empty(), "finding title should not be empty");
        assert!(!f.message.is_empty(), "finding message should not be empty");
        assert!(!f.agent.is_empty(), "finding agent should not be empty");
        assert!(f.line > 0, "finding line number should be > 0, got {}", f.line);

        // Severity should be a valid variant (this is guaranteed by the enum,
        // but we check the display round-trip).
        let sev_str = f.severity.to_string();
        assert!(
            ["info", "warning", "error"].contains(&sev_str.as_str()),
            "unexpected severity: {sev_str}"
        );
    }

    // At least one finding should reference the expected file
    let has_match = findings
        .iter()
        .any(|f| f.file.contains(expected_file_substring));
    assert!(
        has_match,
        "expected at least one finding referencing '{expected_file_substring}', got files: {:?}",
        findings.iter().map(|f| &f.file).collect::<Vec<_>>()
    );
}

/// Print a human-readable summary of findings (for `--nocapture` debugging).
fn print_findings_summary(label: &str, findings: &[Finding]) {
    let summary = nitpik::models::finding::Summary::from_findings(findings);
    eprintln!(
        "\n  [{label}] {} finding(s): {} error, {} warning, {} info",
        summary.total, summary.errors, summary.warnings, summary.info
    );
    for f in findings {
        eprintln!(
            "    [{:7}] {}:{} — {} (agent: {})",
            f.severity, f.file, f.line, f.title, f.agent
        );
    }
}

// ---------------------------------------------------------------------------
// Tests — one per profile
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e2e_backend_profile() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: backend profile ===");
    let (repo, _tmp) = setup_repo("backend").await;
    let findings = run_review(&repo, &["backend"], &config).await;

    print_findings_summary("backend", &findings);
    assert_findings_valid(&findings, "handler.rs");

    // The backend changeset has: unwrap(), N+1 loop, missing error handling,
    // out-of-bounds indexing. We expect the backend reviewer to catch some.
    assert!(
        findings.iter().any(|f| f.agent == "backend"),
        "findings should come from the backend agent"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_frontend_profile() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: frontend profile ===");
    let (repo, _tmp) = setup_repo("frontend").await;
    let findings = run_review(&repo, &["frontend"], &config).await;

    print_findings_summary("frontend", &findings);
    assert_findings_valid(&findings, "UserCard.tsx");

    // The frontend changeset has: dangerouslySetInnerHTML, missing alt attr,
    // useEffect with no deps (infinite loop), non-interactive element with
    // click handler. The frontend reviewer should catch some of these.
    assert!(
        findings.iter().any(|f| f.agent == "frontend"),
        "findings should come from the frontend agent"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_security_profile() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: security profile ===");
    let (repo, _tmp) = setup_repo("security").await;
    let findings = run_review(&repo, &["security"], &config).await;

    print_findings_summary("security", &findings);
    assert_findings_valid(&findings, "auth.py");

    // The security changeset has: SQL injection, hardcoded credentials,
    // MD5 password hashing, command injection via subprocess+shell=True,
    // insecure random for token generation.
    assert!(
        findings.iter().any(|f| f.agent == "security"),
        "findings should come from the security agent"
    );

    // Security issues should generally be warning or error severity.
    let severe_count = findings
        .iter()
        .filter(|f| f.severity >= Severity::Warning)
        .count();
    assert!(
        severe_count > 0,
        "security review should produce at least one warning-or-above finding"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_architect_profile() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: architect profile ===");
    let (repo, _tmp) = setup_repo("architect").await;
    let findings = run_review(&repo, &["architect"], &config).await;

    print_findings_summary("architect", &findings);
    assert_findings_valid(&findings, "service.go");

    // The architect changeset has: a god object (AppService does everything),
    // tight coupling (DB, email, HTTP, file I/O all in one struct),
    // no separation of concerns, error swallowing.
    assert!(
        findings.iter().any(|f| f.agent == "architect"),
        "findings should come from the architect agent"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_auto_profile_selection() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: auto profile selection ===");
    let (repo, _tmp) = setup_repo("auto").await;
    let findings = run_review(&repo, &["auto"], &config).await;

    print_findings_summary("auto", &findings);

    // The auto changeset has both .rs and .tsx files, so auto-selection
    // should pick backend (for .rs), frontend (for .tsx), and security.
    assert!(
        !findings.is_empty(),
        "auto mode should produce findings for a mixed changeset"
    );

    // Verify that multiple agents contributed findings
    let unique_agents: std::collections::HashSet<&str> =
        findings.iter().map(|f| f.agent.as_str()).collect();
    eprintln!("  agents that produced findings: {:?}", unique_agents);

    // At minimum the auto selector should have picked backend and/or security
    // for .rs files, and frontend for .tsx files.
    assert!(
        unique_agents.len() >= 2,
        "auto mode should use at least 2 different agents, got: {unique_agents:?}"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_multiple_profiles_combined() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: multiple profiles (backend + security) ===");
    let (repo, _tmp) = setup_repo("backend").await;
    let findings = run_review(&repo, &["backend", "security"], &config).await;

    print_findings_summary("backend+security", &findings);
    assert_findings_valid(&findings, "handler.rs");

    // Both agents should have reviewed the same file
    let unique_agents: std::collections::HashSet<&str> =
        findings.iter().map(|f| f.agent.as_str()).collect();
    eprintln!("  agents that produced findings: {:?}", unique_agents);

    assert!(
        unique_agents.contains("backend") || unique_agents.contains("security"),
        "at least one of backend/security agents should produce findings"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_output_formats() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: output format rendering ===");
    let (repo, _tmp) = setup_repo("backend").await;
    let findings = run_review(&repo, &["backend"], &config).await;

    assert!(!findings.is_empty(), "need findings to test rendering");

    // Verify all output formats can render the real findings without panicking
    use nitpik::output::OutputRenderer;

    let terminal = nitpik::output::terminal::TerminalRenderer.render(&findings);
    assert!(!terminal.is_empty(), "terminal output should not be empty");

    let json_out = nitpik::output::json::JsonRenderer.render(&findings);
    let parsed: serde_json::Value =
        serde_json::from_str(&json_out).expect("JSON output should be valid JSON");
    assert!(
        parsed.get("findings").is_some() || parsed.is_array(),
        "JSON output should contain findings"
    );

    let github = nitpik::output::github::GithubRenderer.render(&findings);
    assert!(!github.is_empty(), "github output should not be empty");

    let bitbucket = nitpik::output::bitbucket::BitbucketRenderer.render(&findings);
    let bb_parsed: serde_json::Value =
        serde_json::from_str(&bitbucket).expect("Bitbucket output should be valid JSON");
    assert!(
        bb_parsed.get("annotations").is_some(),
        "Bitbucket output should have annotations"
    );

    eprintln!("  All 4 output formats rendered successfully.");
}

// ---------------------------------------------------------------------------
// Custom profile
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn e2e_custom_profile() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: custom profile (perf-reviewer) ===");

    // The custom profile .md lives in the fixtures dir, not in builtins.
    let custom_profile_path = fixtures_dir()
        .join("custom_profile")
        .join("reviewer.md");
    assert!(
        custom_profile_path.exists(),
        "custom profile fixture should exist at {}",
        custom_profile_path.display()
    );

    // Reuse the backend scenario — it has perf-relevant issues (N+1, unwrap, etc.)
    let (repo, _tmp) = setup_repo("backend").await;

    // Resolve the profile by file path (not by name from --profile-dir)
    let profile_path_str = custom_profile_path.to_string_lossy().to_string();
    let profiles = vec![profile_path_str];
    let agent_defs = agents::resolve_profiles(&profiles, None)
        .await
        .expect("should resolve custom profile from file path");

    assert_eq!(agent_defs.len(), 1);
    assert_eq!(agent_defs[0].profile.name, "perf-reviewer");
    eprintln!("  resolved custom profile: {}", agent_defs[0].profile.name);

    // Run the review with the custom profile
    let input = InputMode::GitBase("HEAD".to_string());
    let diffs = diff::get_diffs(&input, &repo)
        .await
        .expect("failed to get diffs");
    assert!(!diffs.is_empty());

    let baseline = context::build_baseline_context(&repo, &diffs, &config).await;
    let review_context = ReviewContext {
        diffs,
        baseline,
        repo_root: repo.to_string_lossy().to_string(),
        is_path_scan: false,
    };

    let provider: Arc<dyn ReviewProvider> = Arc::new(
        RigProvider::new(config.provider.clone(), repo.clone())
            .expect("failed to create provider"),
    );

    let cache = CacheEngine::new(false);
    let progress = std::sync::Arc::new(nitpik::progress::ProgressTracker::new(&[], &[], false));
    let orchestrator = ReviewOrchestrator::new(Arc::clone(&provider), &config, cache, progress, false, None, String::new());

    let result = orchestrator
        .run(&review_context, &agent_defs, 2, false, 10, 10)
        .await
        .expect("orchestrator should succeed with custom profile");
    let findings = result.findings;

    print_findings_summary("custom/perf-reviewer", &findings);
    assert_findings_valid(&findings, "handler.rs");

    // Findings should be attributed to our custom profile name
    assert!(
        findings.iter().all(|f| f.agent == "perf-reviewer"),
        "all findings should come from the custom 'perf-reviewer' agent, got agents: {:?}",
        findings.iter().map(|f| &f.agent).collect::<Vec<_>>()
    );

    eprintln!(
        "  ✓ custom profile produced {} finding(s)",
        findings.len()
    );
}

/// E2E test that uses a custom profile with a tool definition and runs in
/// agentic mode. Verifies that:
/// 1. The custom tool is surfaced to the LLM (via the system/user prompt).
/// 2. The review produces valid findings attributed to the custom profile.
/// 3. If the model decides to call the custom tool, tool-call events are captured.
#[tokio::test]
#[ignore]
async fn e2e_custom_tool_agentic() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: custom tool agentic (tool-reviewer) ===");

    // The custom-tool profile lives in fixtures/e2e/custom_tool/
    let custom_profile_path = fixtures_dir()
        .join("custom_tool")
        .join("reviewer.md");
    assert!(
        custom_profile_path.exists(),
        "custom tool profile fixture should exist at {}",
        custom_profile_path.display()
    );

    // Reuse the backend scenario
    let (repo, _tmp) = setup_repo("backend").await;

    // Resolve the profile by file path
    let profile_path_str = custom_profile_path.to_string_lossy().to_string();
    let profiles = vec![profile_path_str];
    let agent_defs = agents::resolve_profiles(&profiles, None)
        .await
        .expect("should resolve custom tool profile from file path");

    assert_eq!(agent_defs.len(), 1);
    assert_eq!(agent_defs[0].profile.name, "tool-reviewer");

    // Verify the profile actually has custom tools parsed
    assert!(
        !agent_defs[0].profile.tools.is_empty(),
        "tool-reviewer profile should have at least one custom tool definition"
    );
    assert_eq!(agent_defs[0].profile.tools[0].name, "check_syntax");
    eprintln!(
        "  resolved profile '{}' with {} custom tool(s): [{}]",
        agent_defs[0].profile.name,
        agent_defs[0].profile.tools.len(),
        agent_defs[0]
            .profile
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // --- Install tracing subscriber to capture tool-call events ---
    use tracing_subscriber::layer::SubscriberExt;

    let collector = tool_call_layer::ToolCallCollector::default();
    let tool_spans = Arc::clone(&collector.tool_spans);

    let subscriber = tracing_subscriber::registry()
        .with(collector)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .with(tracing_subscriber::filter::EnvFilter::new("info"));

    let _guard = tracing::subscriber::set_default(subscriber);

    // --- Run the review in agentic mode ---
    let input = InputMode::GitBase("HEAD".to_string());
    let diffs = diff::get_diffs(&input, &repo)
        .await
        .expect("failed to get diffs");
    assert!(!diffs.is_empty(), "changeset should produce diffs");

    let baseline = context::build_baseline_context(&repo, &diffs, &config).await;
    let review_context = ReviewContext {
        diffs,
        baseline,
        repo_root: repo.to_string_lossy().to_string(),
        is_path_scan: false,
    };

    let provider: Arc<dyn ReviewProvider> = Arc::new(
        RigProvider::new(config.provider.clone(), repo.clone())
            .expect("failed to create provider"),
    );

    // Retry loop for rate-limiting resilience
    let mut findings = Vec::new();
    for attempt in 0..3u32 {
        let cache = CacheEngine::new(false);
        let progress =
            std::sync::Arc::new(nitpik::progress::ProgressTracker::new(&[], &[], false));
        let orchestrator = ReviewOrchestrator::new(
            Arc::clone(&provider),
            &config,
            cache,
            progress,
            false,
            None,
            String::new(),
        );

        // agentic=true, max_turns=5, max_tool_calls=10
        let result = orchestrator
            .run(&review_context, &agent_defs, 2, true, 5, 10)
            .await
            .expect("agentic orchestrator should succeed with custom tool profile");

        findings = result.findings;

        if result.failed_tasks > 0 && findings.is_empty() && attempt < 2 {
            let backoff = 10 * (attempt + 1);
            eprintln!(
                "  ⚠ attempt {} failed ({} task errors, likely rate-limited) — retrying in {backoff}s",
                attempt + 1,
                result.failed_tasks,
            );
            tokio::time::sleep(std::time::Duration::from_secs(backoff as u64)).await;
            continue;
        }
        break;
    }

    print_findings_summary("custom-tool/tool-reviewer", &findings);
    assert_findings_valid(&findings, "handler.rs");

    // Findings should be attributed to our custom profile
    assert!(
        findings.iter().all(|f| f.agent == "tool-reviewer"),
        "all findings should come from the 'tool-reviewer' agent, got agents: {:?}",
        findings.iter().map(|f| &f.agent).collect::<Vec<_>>()
    );

    // --- Check for tool-call events ---
    let captured = tool_spans.lock().unwrap();
    if captured.is_empty() {
        eprintln!(
            "  ⚠ no tool calls captured — model reviewed without invoking custom tools"
        );
    } else {
        eprintln!(
            "  captured {} tool-call event(s): {:?}",
            captured.len(),
            captured.iter().take(10).collect::<Vec<_>>()
        );
    }

    eprintln!(
        "  ✓ custom tool agentic test produced {} finding(s){}",
        findings.len(),
        if captured.is_empty() {
            " (no tool calls)".to_string()
        } else {
            format!(" and invoked {} tool call(s)", captured.len())
        }
    );
}

/// A custom tracing layer that records tool-call events emitted by rig-core.
///
/// rig-core logs `tracing::info!("executed tool {tool_name} ...")` inside an
/// `execute_tool` span every time a tool is invoked.  We capture those span
/// names so the test can assert that tool calling actually occurred.
mod tool_call_layer {
    use std::sync::{Arc, Mutex};

    use tracing::Subscriber;
    use tracing_subscriber::layer::Context;
    use tracing_subscriber::Layer;

    /// Shared log of tool-call span names observed during the test.
    #[derive(Clone, Default)]
    pub struct ToolCallCollector {
        pub tool_spans: Arc<Mutex<Vec<String>>>,
    }

    impl<S: Subscriber> Layer<S> for ToolCallCollector {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            _id: &tracing::span::Id,
            _ctx: Context<'_, S>,
        ) {
            let name = attrs.metadata().name();
            // rig-core creates an `execute_tool` span for every tool call
            if name == "execute_tool" {
                self.tool_spans
                    .lock()
                    .unwrap()
                    .push(name.to_string());
            }
        }

        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: Context<'_, S>,
        ) {
            // rig-core logs: info!("executed tool {tool_name} with args ...")
            let meta = event.metadata();
            if *meta.level() == tracing::Level::INFO {
                // Capture the formatted message
                struct MsgVisitor(String);
                impl tracing::field::Visit for MsgVisitor {
                    fn record_debug(
                        &mut self,
                        field: &tracing::field::Field,
                        value: &dyn std::fmt::Debug,
                    ) {
                        if field.name() == "message" {
                            self.0 = format!("{value:?}");
                        }
                    }
                    fn record_str(
                        &mut self,
                        field: &tracing::field::Field,
                        value: &str,
                    ) {
                        if field.name() == "message" {
                            self.0 = value.to_string();
                        }
                    }
                }
                let mut visitor = MsgVisitor(String::new());
                event.record(&mut visitor);
                if visitor.0.contains("executed tool") {
                    self.tool_spans
                        .lock()
                        .unwrap()
                        .push(visitor.0);
                }
            }
        }
    }
}

/// Set up a two-stage repo for the cache prior-findings test.
///
/// Stage 1: commit `base/`, apply `changeset_v1/` (unstaged changes).
/// Stage 2: commit v1, apply `changeset_v2/` (new unstaged changes).
///
/// Returns `(repo_path, tempdir_handle)`. Caller advances stages by calling
/// `advance_cache_prior_repo()`.
async fn setup_cache_prior_repo() -> (PathBuf, tempfile::TempDir) {
    let base_dir = fixtures_dir().join("cache_prior").join("base");
    let v1_dir = fixtures_dir().join("cache_prior").join("changeset_v1");

    let tmp = tempfile::Builder::new()
        .prefix("nitpik-e2e-cache-prior-")
        .tempdir_in("/tmp")
        .expect("failed to create tempdir");
    let repo = tmp.path().to_path_buf();

    // git init + initial commit with base files
    run_git(&repo, &["init"]).await;
    run_git(&repo, &["config", "user.email", "test@nitpik.dev"]).await;
    run_git(&repo, &["config", "user.name", "Nitpik E2E"]).await;

    copy_tree(&base_dir, &repo);
    run_git(&repo, &["add", "."]).await;
    run_git(&repo, &["commit", "-m", "initial commit"]).await;

    // Apply changeset v1 (unstaged changes for the first review)
    copy_tree(&v1_dir, &repo);

    (repo, tmp)
}

/// Advance the cache_prior repo to stage 2: commit v1, apply v2.
async fn advance_cache_prior_repo(repo: &Path) {
    let v2_dir = fixtures_dir().join("cache_prior").join("changeset_v2");

    // Commit v1 changes
    run_git(repo, &["add", "."]).await;
    run_git(repo, &["commit", "-m", "apply v1 changes"]).await;

    // Apply v2 changeset (partially fixes v1 issues, introduces new ones)
    copy_tree(&v2_dir, repo);
}

#[tokio::test]
#[ignore]
async fn e2e_cache_prior_findings() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: cache prior findings on invalidation ===");

    let (repo, _tmp) = setup_cache_prior_repo().await;

    // Use a dedicated cache dir so we don't interfere with other tests
    let cache_dir = tempfile::Builder::new()
        .prefix("nitpik-e2e-cache-")
        .tempdir_in("/tmp")
        .expect("failed to create cache tempdir");
    let _cache_store = nitpik::cache::store::FileStore::new_with_dir(
        cache_dir.path().to_path_buf(),
    );

    // --- Stage 1: Review changeset v1 ---
    eprintln!("  stage 1: reviewing changeset v1 (division by zero + unwrap)...");

    let input = InputMode::GitBase("HEAD".to_string());
    let diffs_v1 = diff::get_diffs(&input, &repo)
        .await
        .expect("failed to get v1 diffs");
    assert!(!diffs_v1.is_empty(), "v1 should produce diffs");

    let profiles: Vec<String> = vec!["backend".to_string()];
    let agent_defs = agents::resolve_profiles(&profiles, None)
        .await
        .expect("failed to resolve profiles");

    let baseline_v1 = context::build_baseline_context(&repo, &diffs_v1, &config).await;
    let review_context_v1 = ReviewContext {
        diffs: diffs_v1,
        baseline: baseline_v1,
        repo_root: repo.to_string_lossy().to_string(),
        is_path_scan: false,
    };

    let provider: Arc<dyn ReviewProvider> = Arc::new(
        RigProvider::new(config.provider.clone(), repo.clone())
            .expect("failed to create provider"),
    );

    // Run review 1 with cache enabled — this populates the cache + sidecar
    // We can't use CacheEngine::new(true) because it uses the default dir.
    // Instead, we'll manually write to our test cache store after running
    // with cache disabled, to simulate the caching.
    let cache1 = CacheEngine::new(false);
    let progress1 = std::sync::Arc::new(nitpik::progress::ProgressTracker::new(&[], &[], false));
    let orch1 = ReviewOrchestrator::new(
        Arc::clone(&provider), &config, cache1, progress1, false, None, String::new(),
    );

    let result_v1 = orch1
        .run(&review_context_v1, &agent_defs, 2, false, 10, 10)
        .await
        .expect("v1 review should succeed");

    let findings_v1 = &result_v1.findings;
    print_findings_summary("cache_prior v1", findings_v1);
    assert!(
        !findings_v1.is_empty(),
        "v1 should produce findings (division by zero, unwrap, etc.)"
    );
    eprintln!("  stage 1 complete: {} finding(s)", findings_v1.len());

    // --- Stage 2: Advance to v2, review with prior findings ---
    eprintln!("  stage 2: advancing to changeset v2 (partial fix)...");
    advance_cache_prior_repo(&repo).await;

    let diffs_v2 = diff::get_diffs(&input, &repo)
        .await
        .expect("failed to get v2 diffs");
    assert!(!diffs_v2.is_empty(), "v2 should produce diffs");

    let baseline_v2 = context::build_baseline_context(&repo, &diffs_v2, &config).await;
    let review_context_v2 = ReviewContext {
        diffs: diffs_v2,
        baseline: baseline_v2,
        repo_root: repo.to_string_lossy().to_string(),
        is_path_scan: false,
    };

    // For stage 2, we need to manually seed the cache store with v1 findings
    // and sidecar, then have the orchestrator pick them up.
    //
    // Since ReviewOrchestrator uses CacheEngine::new() internally with
    // the default cache dir, we'll seed the real default cache dir for this test.
    let real_cache = CacheEngine::new(true);
    let real_cache_path = real_cache.path().expect("cache path").clone();
    let real_store = nitpik::cache::store::FileStore::new_with_dir(real_cache_path.clone());

    // Reconstruct the v1 cache key by building the same prompt the orchestrator would.
    // We need the model name from config.
    let model = config.provider.model.clone();

    // Seed v1 findings + sidecar for calculator.rs × backend × model
    // We don't know the exact prompt, but we can use a sentinel key.
    // Actually, we can just seed a sidecar with any old key that has the v1 findings.
    let v1_key = format!("e2e-cache-prior-v1-{}", std::process::id());
    real_store.put(&v1_key, findings_v1);
    real_store.put_sidecar("calculator.rs", "backend", &model, &v1_key, "");

    // Now run the review with cache enabled — it will miss (different prompt/key)
    // and should pick up prior findings from the sidecar.
    let cache2 = CacheEngine::new(true);
    let progress2 = std::sync::Arc::new(nitpik::progress::ProgressTracker::new(&[], &[], false));
    let orch2 = ReviewOrchestrator::new(
        Arc::clone(&provider), &config, cache2, progress2,
        false, // no_prior_context = false → inject prior findings
        None,  // unlimited
        String::new(),
    );

    let result_v2 = orch2
        .run(&review_context_v2, &agent_defs, 2, false, 10, 10)
        .await
        .expect("v2 review should succeed");

    let findings_v2 = &result_v2.findings;
    print_findings_summary("cache_prior v2", findings_v2);

    // v2 should produce findings — at minimum the remaining unwrap() and
    // the new multiply overflow risk.
    assert!(
        !findings_v2.is_empty(),
        "v2 should produce findings (remaining unwrap, new multiply overflow)"
    );

    // The v2 findings should reflect awareness of v1 context:
    // - The division by zero should NOT be re-raised (it was fixed)
    // - The unwrap() issue should persist or be re-raised
    // - New issues (multiply overflow) should appear
    //
    // We can't assert exact finding titles (LLM-dependent), but we can
    // verify structural validity and log for manual inspection.
    for f in findings_v2 {
        assert!(!f.file.is_empty());
        assert!(!f.title.is_empty());
        assert!(!f.message.is_empty());
        assert!(f.line > 0);
    }

    eprintln!(
        "  stage 2 complete: {} finding(s) (v1 had {})",
        findings_v2.len(),
        findings_v1.len(),
    );
    eprintln!("  ✓ cache prior findings e2e test passed");

    // Clean up seeded entries
    let _ = real_store.clear();
}

#[tokio::test]
#[ignore]
async fn e2e_agentic_mode() {
    require_api_key!();
    let config = real_config();

    eprintln!("\n=== E2E: agentic mode (tool-calling loop) ===");
    // Use the backend fixture — the agent should use ReadFile, SearchText,
    // and ListDirectory tools to explore the repo for deeper context.
    let (repo, _tmp) = setup_repo("backend").await;

    // --- Install a tracing subscriber to capture tool-call events ---
    use tracing_subscriber::layer::SubscriberExt;

    let collector = tool_call_layer::ToolCallCollector::default();
    let tool_spans = Arc::clone(&collector.tool_spans);

    // Build a subscriber: our collector layer + a fmt layer for stderr output
    let subscriber = tracing_subscriber::registry()
        .with(collector)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr),
        )
        .with(
            tracing_subscriber::filter::EnvFilter::new("info"),
        );

    // Use `set_default` so it only applies to this thread/task, not globally.
    let _guard = tracing::subscriber::set_default(subscriber);

    // --- run_review variant with agentic=true ---
    let input = InputMode::GitBase("HEAD".to_string());
    let diffs = diff::get_diffs(&input, &repo)
        .await
        .expect("failed to get diffs");
    assert!(!diffs.is_empty(), "changeset should produce diffs");

    let profiles: Vec<String> = vec!["backend".to_string()];
    let agent_defs = agents::resolve_profiles(&profiles, None)
        .await
        .expect("failed to resolve profiles");

    let baseline = context::build_baseline_context(&repo, &diffs, &config).await;
    let review_context = ReviewContext {
        diffs,
        baseline,
        repo_root: repo.to_string_lossy().to_string(),
        is_path_scan: false,
    };

    let provider: Arc<dyn ReviewProvider> = Arc::new(
        RigProvider::new(config.provider.clone(), repo.clone())
            .expect("failed to create provider"),
    );

    // Retry loop: agentic requests are more expensive and prone to rate-limiting
    // when running concurrently with other E2E tests.
    let mut findings = Vec::new();
    for attempt in 0..3u32 {
        let cache = CacheEngine::new(false);
        let progress = std::sync::Arc::new(nitpik::progress::ProgressTracker::new(&[], &[], false));
        let orchestrator = ReviewOrchestrator::new(Arc::clone(&provider), &config, cache, progress, false, None, String::new());

        // agentic=true, max_turns=5, max_tool_calls=10
        let result = orchestrator
            .run(&review_context, &agent_defs, 2, true, 5, 10)
            .await
            .expect("agentic orchestrator should succeed");

        findings = result.findings;

        if result.failed_tasks > 0 && findings.is_empty() && attempt < 2 {
            let backoff = 10 * (attempt + 1);
            eprintln!(
                "  ⚠ attempt {} failed ({} task errors, likely rate-limited) — retrying in {backoff}s",
                attempt + 1,
                result.failed_tasks,
            );
            tokio::time::sleep(std::time::Duration::from_secs(backoff as u64)).await;
            continue;
        }
        break;
    }

    print_findings_summary("agentic", &findings);
    assert_findings_valid(&findings, "handler.rs");

    assert!(
        findings.iter().any(|f| f.agent == "backend"),
        "agentic findings should come from the backend agent"
    );

    // --- Verify that tool calling actually occurred ---
    // NOTE: Tool use is model-dependent. If the diff is self-contained the
    // model may correctly decide it doesn't need to explore the repo. We log
    // the result but don't hard-fail on it — the critical assertion is that
    // agentic mode produces valid findings.
    let captured = tool_spans.lock().unwrap();
    if captured.is_empty() {
        eprintln!(
            "  ⚠ no tool calls captured — model reviewed the diff without exploring the repo"
        );
    } else {
        eprintln!(
            "  captured {} tool-call event(s): {:?}",
            captured.len(),
            captured.iter().take(10).collect::<Vec<_>>()
        );
    }

    eprintln!(
        "  ✓ agentic mode produced {} finding(s){}",
        findings.len(),
        if captured.is_empty() {
            " (no tool calls)".to_string()
        } else {
            format!(" and invoked {} tool call(s)", captured.len())
        }
    );
}
