//! nitpik — AI-powered code review CLI.
//!
//! Entry point and error handling boundary. Uses `anyhow` for
//! ergonomic error propagation and user-facing messages.

mod cli;

use nitpik::agents;
use nitpik::cache;
use nitpik::config;
use nitpik::constants;
use nitpik::context;
use nitpik::diff;
use nitpik::env;
use nitpik::license;
use nitpik::models;
use nitpik::orchestrator;
use nitpik::progress;
use nitpik::providers;
use nitpik::security;
use nitpik::telemetry;
use nitpik::threat;
use nitpik::update;

use std::io::IsTerminal;
use std::path::Path;
use std::process;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;

use cli::args::{CacheAction, Cli, Command, LicenseAction, OutputFormat, UpdateArgs};
use config::Config;
use env::Env;
use models::{DEFAULT_PROFILE, Severity};
use progress::ProgressTracker;
use providers::ReviewProvider;
use providers::rig::RigProvider;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        use colored::Colorize;
        eprintln!("{} {err:#}", "Error:".red().bold());
        process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    let no_telemetry = cli.no_telemetry;

    match cli.command {
        Command::Review(args) => run_review(*args, no_telemetry).await,
        Command::Profiles(args) => run_profiles(args).await,
        Command::Validate(args) => run_validate(args).await,
        Command::Cache { action } => run_cache(action).await,
        Command::License { action } => run_license(action).await,
        Command::Update(args) => run_update(args).await,
        Command::Version => run_version(),
    }
}

/// Print detailed version and build information.
fn run_version() -> Result<()> {
    use colored::Colorize;

    println!("{} {}", "nitpik".bold(), constants::VERSION.green().bold());
    println!("{}     {}", "commit:".dimmed(), constants::GIT_SHA);
    println!("{}      {}", "built:".dimmed(), constants::BUILD_DATE);
    println!("{}     {}", "target:".dimmed(), constants::TARGET);
    Ok(())
}

/// List available agent profiles.
async fn run_profiles(args: cli::args::ProfilesArgs) -> Result<()> {
    use colored::Colorize;

    let agents = agents::list_all_profiles(args.profile_dir.as_deref())
        .await
        .context("failed to list profiles")?;

    if agents.is_empty() {
        println!("No profiles found.");
        return Ok(());
    }

    for agent in &agents {
        let p = &agent.profile;
        println!("  {}  {}", p.name.bold(), p.description.dimmed(),);

        if !p.tags.is_empty() {
            println!("         {}  {}", "tags:".cyan(), p.tags.join(", "));
        }
        if let Some(ref model) = p.model {
            println!("         {}  {}", "model:".cyan(), model);
        }
        if !p.tools.is_empty() {
            let tool_names: Vec<_> = p.tools.iter().map(|t| t.name.as_str()).collect();
            println!("         {}  {}", "tools:".cyan(), tool_names.join(", "));
        }
    }

    Ok(())
}

/// Validate a custom agent profile markdown file.
async fn run_validate(args: cli::args::ValidateArgs) -> Result<()> {
    let path = &args.file;
    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;

    match agents::parser::parse_agent_definition(&content) {
        Ok(agent) => {
            use colored::Colorize;
            let p = &agent.profile;
            println!(
                "  {} {}  {}",
                "✔".green().bold(),
                p.name.bold(),
                p.description.dimmed(),
            );

            if !p.tags.is_empty() {
                println!("         {}  {}", "tags:".cyan(), p.tags.join(", "));
            }
            if let Some(ref model) = p.model {
                println!("         {}  {}", "model:".cyan(), model);
            }
            if !p.tools.is_empty() {
                let tool_names: Vec<_> = p.tools.iter().map(|t| t.name.as_str()).collect();
                println!("         {}  {}", "tools:".cyan(), tool_names.join(", "));
            }
            println!(
                "         {}  {} chars",
                "prompt:".cyan(),
                agent.system_prompt.len()
            );
            Ok(())
        }
        Err(e) => {
            use colored::Colorize;
            bail!(
                "{} {}",
                "✖".red().bold(),
                format!("Invalid profile: {e}").red()
            );
        }
    }
}

/// Manage the result cache.
async fn run_cache(action: CacheAction) -> Result<()> {
    let engine = cache::CacheEngine::new(true);

    match action {
        CacheAction::Clear => {
            let stats = engine.clear().await.context("failed to clear cache")?;
            println!(
                "Cleared {} cached entry/entries ({}).",
                stats.entries,
                stats.human_size(),
            );
        }
        CacheAction::Stats => {
            let stats = engine.stats().await.context("failed to read cache stats")?;
            println!("Cache entries: {}", stats.entries);
            println!("Cache size:    {}", stats.human_size());
        }
        CacheAction::Path => match engine.path() {
            Some(p) => println!("{}", p.display()),
            None => bail!("cache directory could not be determined"),
        },
    }

    Ok(())
}

/// Update nitpik to the latest release from GitHub.
async fn run_update(args: UpdateArgs) -> Result<()> {
    update::run_update(args.force)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Manage the commercial license key.
async fn run_license(action: LicenseAction) -> Result<()> {
    use colored::Colorize;

    match action {
        LicenseAction::Activate { key } => {
            // Verify the key first
            let claims = license::verify_license_key(&key).context("invalid license key")?;
            let expiry =
                license::check_expiry(&claims).context("failed to check license expiry")?;

            if expiry == license::ExpiryStatus::Expired {
                bail!("this license key has expired ({})", claims.expires_at);
            }

            // Write to global config
            let config_dir = dirs::config_dir()
                .map(|d| d.join("nitpik"))
                .context("could not determine config directory")?;
            std::fs::create_dir_all(&config_dir)?;
            let config_path = config_dir.join("config.toml");

            // Load existing config or start fresh
            let mut content = if config_path.exists() {
                std::fs::read_to_string(&config_path)?
            } else {
                String::new()
            };

            // Append or replace the [license] section
            if let Some(start) = content.find("[license]") {
                let rest = &content[start..];
                let end = rest[1..]
                    .find("\n[")
                    .map(|i| start + 1 + i)
                    .unwrap_or(content.len());
                content.replace_range(start..end, &format!("[license]\nkey = \"{key}\"\n"));
            } else {
                content.push_str(&format!("\n[license]\nkey = \"{key}\"\n"));
            }

            std::fs::write(&config_path, &content)?;

            println!(
                "  {} License activated for {} (expires {})",
                "✔".green().bold(),
                claims.customer_name.bold(),
                claims.expires_at,
            );
        }
        LicenseAction::Status => {
            let config =
                Config::load(None, &Env::real()).context("failed to load configuration")?;

            match config.license.key {
                Some(ref key) => match license::verify_license_key(key) {
                    Ok(claims) => {
                        let expiry =
                            license::check_expiry(&claims).context("failed to check expiry")?;
                        println!("  {}  {}", "Customer:".cyan(), claims.customer_name);
                        println!("  {}       {}", "ID:".cyan(), claims.customer_id);
                        println!("  {}  {}", "Issued at:".cyan(), claims.issued_at);
                        println!("  {} {}", "Expires at:".cyan(), claims.expires_at);
                        match expiry {
                            license::ExpiryStatus::Valid => {
                                println!("  {}    {}", "Status:".cyan(), "valid".green());
                            }
                            license::ExpiryStatus::ExpiringSoon { days } => {
                                println!(
                                    "  {}    {}",
                                    "Status:".cyan(),
                                    format!("expires in {days} day(s)").yellow(),
                                );
                            }
                            license::ExpiryStatus::Expired => {
                                println!("  {}    {}", "Status:".cyan(), "expired".red());
                            }
                        }
                    }
                    Err(e) => {
                        println!(
                            "  {} {}",
                            "✖".red().bold(),
                            format!("Invalid license key: {e}").red(),
                        );
                    }
                },
                None => {
                    println!("  No license key configured.");
                    println!("  Use `nitpik license activate <KEY>` to add one.");
                }
            }
        }
        LicenseAction::Deactivate => {
            let config_dir = dirs::config_dir()
                .map(|d| d.join("nitpik"))
                .context("could not determine config directory")?;
            let config_path = config_dir.join("config.toml");

            if config_path.exists() {
                let mut content = std::fs::read_to_string(&config_path)?;
                if let Some(start) = content.find("[license]") {
                    let rest = &content[start..];
                    let end = rest[1..]
                        .find("\n[")
                        .map(|i| start + 1 + i)
                        .unwrap_or(content.len());
                    content.replace_range(start..end, "");
                    std::fs::write(&config_path, content.trim_end())?;
                }
            }

            println!("  {} License key removed.", "✔".green().bold(),);
        }
    }

    Ok(())
}
/// Canonicalize the `--path` directory and locate the git repo root.
async fn resolve_repo_root(path: &Path) -> Result<String> {
    let base_dir = std::fs::canonicalize(path)
        .with_context(|| format!("--path directory not found: {}", path.display()))?;
    match diff::git::find_repo_root(&base_dir).await {
        Ok(root) => Ok(root),
        Err(_) => Ok(base_dir.display().to_string()),
    }
}

/// Fetch the commit log for baseline context, if applicable.
async fn build_commit_log(
    no_commit_context: bool,
    input_mode: &models::InputMode,
    repo_root_path: &Path,
) -> Vec<String> {
    if no_commit_context {
        return Vec::new();
    }
    if let models::InputMode::GitBase(base_ref) = input_mode {
        diff::git::git_log(repo_root_path, base_ref, 50)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// Build the provider, cache engine, and review orchestrator.
async fn create_orchestrator(
    config: &Config,
    repo_root_path: &Path,
    no_cache: bool,
    progress: Arc<dyn progress::ProgressReporter>,
    no_prior_context: bool,
    max_prior_findings: Option<usize>,
) -> Result<(Arc<dyn ReviewProvider>, orchestrator::ReviewOrchestrator)> {
    let provider: Arc<dyn ReviewProvider> = Arc::new(
        RigProvider::new(config.provider.clone(), repo_root_path.to_path_buf())
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    let review_scope = diff::git::detect_branch(repo_root_path, &Env::real()).await;
    let cache = cache::CacheEngine::new(!no_cache);
    let stale_age = std::time::Duration::from_secs(30 * 24 * 60 * 60);
    let _removed = cache.cleanup_stale(stale_age).await;

    let orchestrator = orchestrator::ReviewOrchestrator::new(
        Arc::clone(&provider),
        config,
        cache,
        progress,
        no_prior_context,
        max_prior_findings,
        review_scope,
    );
    Ok((provider, orchestrator))
}

async fn run_review(args: cli::args::ReviewArgs, no_telemetry: bool) -> Result<()> {
    let input_mode = args.validate_input().map_err(|e| anyhow::anyhow!("{e}"))?;
    let repo_root = resolve_repo_root(&args.path).await?;
    let repo_root_path = Path::new(&repo_root);

    let config =
        Config::load(Some(repo_root_path), &Env::real()).context("failed to load configuration")?;
    let license_claims = verify_license(&config);

    let use_agent = args.agent || config.review.agentic.enabled;
    let scan_secrets = args.scan_secrets || config.secrets.enabled;
    let scan_threats = args.scan_threats || config.threats.enabled;

    // Get diff source — keeps raw content alive so parsed diffs
    // can borrow via Cow (zero-copy).
    let diff_source = diff::get_diff_source(&input_mode, repo_root_path)
        .await
        .context("failed to get diffs")?;

    let parsed_diffs;
    let diffs: &[models::FileDiff<'_>] = match &diff_source {
        diff::DiffSource::Raw(content) => {
            parsed_diffs = diff::parser::parse_unified_diff(content);
            &parsed_diffs
        }
        diff::DiffSource::Scanned(d) => d,
    };
    if diffs.is_empty() {
        eprintln!("No changes to review.");
        return Ok(());
    }

    let commit_log = build_commit_log(args.no_commit_context, &input_mode, repo_root_path).await;
    let baseline = context::build_baseline_context(
        repo_root_path,
        diffs,
        &config,
        args.no_project_docs,
        &args.exclude_doc,
        commit_log,
    )
    .await;

    let agent_defs = resolve_agents(&args, &config, diffs, repo_root_path).await?;

    // Debug-only: dump constructed prompts and exit without calling the LLM.
    #[cfg(debug_assertions)]
    if args.debug_prompt {
        use nitpik::diff::chunker;
        use nitpik::orchestrator::prompt::build_prompt;

        let review_ctx = models::context::ReviewContext {
            diffs: diffs.to_vec(),
            baseline,
            repo_root: repo_root.clone(),
            is_path_scan: matches!(input_mode, models::InputMode::DirectPath(_)),
        };

        for agent in &agent_defs {
            for d in diffs {
                if d.is_binary {
                    continue;
                }
                let chunks = chunker::chunk_diff(d, None);
                for chunk in chunks {
                    let user_prompt =
                        build_prompt(&chunk, &review_ctx, agent, &agent_defs, None, use_agent);
                    println!("═══ {} × {} ═══", chunk.path(), agent.profile.name);
                    println!("── system prompt ──");
                    println!("{}", agent.system_prompt);
                    println!("── user prompt ──");
                    println!("{user_prompt}");
                    println!();
                }
            }
        }
        return Ok(());
    }

    let heartbeat = fire_telemetry(&config, diffs, &agent_defs, &license_claims, no_telemetry);

    let progress = setup_progress(&args, diffs, &agent_defs, &baseline, &license_claims);
    progress.start();

    let is_path_scan = matches!(input_mode, models::InputMode::DirectPath(_));
    let (review_context, secret_findings) = build_review_context(
        &args,
        &config,
        diffs,
        baseline,
        &repo_root,
        scan_secrets,
        is_path_scan,
    )?;

    let (provider, orchestrator) = create_orchestrator(
        &config,
        repo_root_path,
        args.no_cache,
        Arc::clone(&progress) as Arc<dyn progress::ProgressReporter>,
        args.no_prior_context,
        args.max_prior_findings,
    )
    .await?;

    let review_result = orchestrator
        .run(
            &review_context,
            &agent_defs,
            args.max_concurrent,
            use_agent,
            args.max_turns,
            args.max_tool_calls,
        )
        .await
        .context("review failed")?;

    // Finalize the live progress display before printing threat scanner status.
    progress.finish();

    // Threat scanning (pattern scan then optional LLM triage)
    let threat_findings = if scan_threats {
        let mut threat_rules = threat::rules::default_rules();
        let config_threat_path: Option<std::path::PathBuf> = config
            .threats
            .additional_rules
            .as_ref()
            .map(std::path::PathBuf::from);
        let threat_rules_path = args
            .threat_rules
            .as_deref()
            .or(config_threat_path.as_deref());
        if let Some(path) = threat_rules_path {
            match threat::rules::load_rules_from_file(path) {
                Ok(extra) => threat_rules.extend(extra),
                Err(e) => eprintln!("Warning: failed to load threat rules: {e}"),
            }
        }

        // Phase 1: fast pattern matching (regex + entropy)
        let raw_matches = threat::scanner::scan_for_threats(
            &review_context.diffs,
            &review_context.baseline.file_contents,
            &threat_rules,
        );

        if raw_matches.is_empty() {
            vec![]
        } else {
            let show_progress = !args.quiet
                && args.format == OutputFormat::Terminal
                && std::io::stderr().is_terminal();

            if show_progress {
                use colored::Colorize;
                use std::io::Write;
                let stderr = std::io::stderr();
                let mut handle = stderr.lock();
                let _ = writeln!(
                    handle,
                    "  {} {}",
                    "▸".cyan().bold(),
                    format!(
                        "Threat scanner: {} pattern match{} found, triaging with LLM…",
                        raw_matches.len(),
                        if raw_matches.len() == 1 { "" } else { "es" }
                    )
                    .dimmed(),
                );
                let _ = handle.flush();
            }

            // Phase 2: LLM triage (fail-open)
            let triaged = threat::triage::triage_findings(
                raw_matches,
                &review_context.baseline.file_contents,
                provider.as_ref(),
            )
            .await;

            if show_progress {
                use colored::Colorize;
                use std::io::Write;
                let stderr = std::io::stderr();
                let mut handle = stderr.lock();
                let _ = writeln!(
                    handle,
                    "  {} {}",
                    "✔".green().bold(),
                    format!(
                        "Threat triage complete: {} finding{} after triage",
                        triaged.len(),
                        if triaged.len() == 1 { "" } else { "s" }
                    )
                    .dimmed(),
                );
                let _ = writeln!(handle);
                let _ = handle.flush();
            }

            triaged.iter().map(threat::match_to_finding).collect()
        }
    } else {
        vec![]
    };

    let mut findings = review_result.findings;
    findings.extend(secret_findings);
    findings.extend(threat_findings);
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });

    let fail_on_severity: Option<Severity> = if args.no_fail {
        None
    } else {
        args.fail_on
            .or(config.review.fail_on)
            .or(Some(Severity::Error))
    };
    render_and_output(&args.format, &findings, fail_on_severity).await;

    // Ensure the telemetry POST completes before the runtime shuts down.
    if let Some(h) = heartbeat {
        let _ = h.await;
    }

    determine_exit(
        &findings,
        fail_on_severity,
        &args.format,
        review_result.failed_tasks,
    )
}

/// Verify the license key from config, returning claims and optional
/// days-until-expiry. Exits the process if the license has expired.
fn verify_license(config: &Config) -> Option<(license::LicenseClaims, Option<i64>)> {
    let key = config.license.key.as_ref()?;
    match license::verify_license_key(key) {
        Ok(claims) => match license::check_expiry(&claims) {
            Ok(license::ExpiryStatus::Expired) => {
                use colored::Colorize;
                eprintln!(
                    "\n  {} {}\n  {}\n",
                    "✖".red().bold(),
                    "Your nitpik license has expired.".red(),
                    "Renew at https://nitpik.dev or contact support.".dimmed(),
                );
                std::process::exit(1);
            }
            Ok(license::ExpiryStatus::ExpiringSoon { days }) => Some((claims, Some(days))),
            Ok(license::ExpiryStatus::Valid) => Some((claims, None)),
            Err(e) => {
                eprintln!("Warning: could not check license expiry: {e}");
                Some((claims, None))
            }
        },
        Err(e) => {
            eprintln!("Warning: invalid license key: {e}");
            None
        }
    }
}

/// Fire anonymous telemetry heartbeat (non-blocking, fails silently).
///
/// Returns a [`tokio::task::JoinHandle`] that the caller should `.await`
/// before the process exits to guarantee the POST completes. The handle
/// is safe to await — it will never return an error that should halt the
/// review.
fn fire_telemetry(
    config: &Config,
    diffs: &[models::FileDiff<'_>],
    agents: &[models::AgentDefinition],
    license_claims: &Option<(license::LicenseClaims, Option<i64>)>,
    no_telemetry: bool,
) -> Option<tokio::task::JoinHandle<()>> {
    if !config.telemetry.enabled || no_telemetry {
        return None;
    }
    use models::diff::DiffLineType;
    let diff_lines: usize = diffs
        .iter()
        .flat_map(|d| d.hunks.iter())
        .flat_map(|h| h.lines.iter())
        .filter(|l| l.line_type == DiffLineType::Added || l.line_type == DiffLineType::Removed)
        .count();
    let payload = telemetry::HeartbeatPayload::from_review(
        diffs.len(),
        diff_lines,
        agents.len(),
        license_claims.is_some(),
    );
    Some(telemetry::send_heartbeat(payload))
}

/// Build the progress tracker, print the banner and informational messages.
fn setup_progress(
    args: &cli::args::ReviewArgs,
    diffs: &[models::FileDiff<'_>],
    agents: &[models::AgentDefinition],
    baseline: &models::BaselineContext,
    license_claims: &Option<(license::LicenseClaims, Option<i64>)>,
) -> Arc<ProgressTracker> {
    let is_interactive = args.format == OutputFormat::Terminal && std::io::stderr().is_terminal();
    let show_info = !args.quiet && is_interactive;
    let show_progress = !args.quiet && is_interactive;

    let file_names: Vec<String> = diffs.iter().map(|d| d.path().to_string()).collect();
    let agent_names: Vec<String> = agents.iter().map(|a| a.profile.name.clone()).collect();
    let progress = Arc::new(ProgressTracker::new(
        &file_names,
        &agent_names,
        show_progress,
    ));

    if show_info {
        let claims_ref = license_claims.as_ref().map(|(c, _)| c);
        cli::print_banner(claims_ref);

        if let Some((_, Some(days))) = license_claims {
            use colored::Colorize;
            use std::io::Write;
            let stderr = std::io::stderr();
            let mut handle = stderr.lock();
            if *days <= 13 {
                let _ = writeln!(
                    handle,
                    "  {} {}",
                    "⚠".yellow().bold(),
                    format!("License expires in {days} day(s) — renew at https://nitpik.dev")
                        .yellow(),
                );
            } else {
                let _ = writeln!(
                    handle,
                    "  {} {}",
                    "ℹ".dimmed(),
                    format!("License expires in {days} days").dimmed(),
                );
            }
            let _ = writeln!(handle);
            let _ = handle.flush();
        }

        if !baseline.project_docs.is_empty() {
            use colored::Colorize;
            use std::io::Write;
            let stderr = std::io::stderr();
            let mut handle = stderr.lock();
            let doc_names: Vec<&str> = baseline.project_docs.keys().map(|s| s.as_str()).collect();
            let _ = writeln!(
                handle,
                "  {} {}",
                "project context:".dimmed(),
                doc_names.join(", ").dimmed(),
            );
            let _ = writeln!(handle);
            let _ = handle.flush();
        }
    }

    progress
}

/// Check findings against the fail-on threshold and task failures.
fn determine_exit(
    findings: &[models::finding::Finding],
    fail_on: Option<Severity>,
    format: &OutputFormat,
    failed_tasks: usize,
) -> Result<()> {
    if let Some(threshold) = fail_on {
        let failing: Vec<_> = findings
            .iter()
            .filter(|f| f.severity >= threshold)
            .collect();
        if !failing.is_empty() {
            if *format == OutputFormat::Terminal {
                eprintln!();
            } else {
                let summary = models::finding::Summary::from_findings(findings);
                eprintln!(
                    "\nReview complete: {} error(s), {} warning(s), {} info — failing on {threshold}+",
                    summary.errors, summary.warnings, summary.info,
                );
            }
            bail!(
                "found {} finding(s) at or above {threshold} threshold",
                failing.len(),
            );
        }
    }
    if failed_tasks > 0 {
        bail!("{failed_tasks} review task(s) failed after retries — results are incomplete");
    }
    Ok(())
}

/// Resolve agent profiles from CLI args and config.
async fn resolve_agents(
    args: &cli::args::ReviewArgs,
    config: &Config,
    diffs: &[models::FileDiff<'_>],
    repo_root_path: &Path,
) -> Result<Vec<models::AgentDefinition>> {
    let profile_names = if args.profile == vec![DEFAULT_PROFILE.to_string()] {
        // CLI default — check config for overrides
        if !config.review.default_profiles.is_empty() {
            config.review.default_profiles.clone()
        } else {
            args.profile.clone()
        }
    } else {
        args.profile.clone()
    };

    let profiles = if profile_names.iter().any(|p| p == "auto") {
        agents::auto::auto_select_profiles(diffs, repo_root_path)
    } else {
        profile_names
    };

    let mut agent_defs = agents::resolve_profiles(&profiles, args.profile_dir.as_deref())
        .await
        .context("failed to resolve agent profiles")?;

    // --tag: add any profiles that match the requested tags
    if !args.tag.is_empty() {
        let by_tag = agents::resolve_profiles_by_tags(&args.tag, args.profile_dir.as_deref())
            .await
            .context("failed to resolve profiles by tag")?;

        // Merge, avoiding duplicates (by profile name)
        let existing_names: std::collections::HashSet<String> =
            agent_defs.iter().map(|a| a.profile.name.clone()).collect();
        for agent in by_tag {
            if !existing_names.contains(&agent.profile.name) {
                agent_defs.push(agent);
            }
        }
    }

    Ok(agent_defs)
}

/// Build review context, optionally scanning and redacting secrets.
fn build_review_context<'a>(
    args: &cli::args::ReviewArgs,
    config: &Config,
    diffs: &[models::FileDiff<'a>],
    baseline: models::BaselineContext,
    repo_root: &str,
    scan_secrets: bool,
    is_path_scan: bool,
) -> Result<(models::ReviewContext<'a>, Vec<models::finding::Finding>)> {
    if !scan_secrets {
        let ctx = models::ReviewContext {
            diffs: diffs.to_vec(),
            baseline,
            repo_root: repo_root.to_string(),
            is_path_scan,
        };
        return Ok((ctx, Vec::new()));
    }

    let mut rules = security::rules::default_rules();

    // Load additional rules: CLI flag takes priority, then config
    let config_rules_path: Option<std::path::PathBuf> = config
        .secrets
        .additional_rules
        .as_ref()
        .map(std::path::PathBuf::from);
    let rules_path = args
        .secrets_rules
        .as_deref()
        .or(config_rules_path.as_deref());
    if let Some(rules_path) = rules_path {
        let extra = security::rules::load_rules_from_file(rules_path)
            .map_err(|e| anyhow::anyhow!("failed to load secret rules: {e}"))?;
        rules.extend(extra);
    }

    // Resolve secrets severity: CLI flag > config > default (warning)
    let secrets_severity = args.secrets_severity.unwrap_or(config.secrets.severity);

    // Scan and redact baseline file contents
    let mut secret_findings = Vec::new();
    let mut redacted_contents = indexmap::IndexMap::new();
    for (path, content) in &baseline.file_contents {
        let (redacted, findings) =
            security::scan_and_redact(content, path, &rules, secrets_severity);
        secret_findings.extend(findings);
        redacted_contents.insert(path.clone(), redacted);
    }

    let ctx = models::ReviewContext {
        diffs: diffs.to_vec(),
        baseline: models::BaselineContext {
            file_contents: redacted_contents,
            project_docs: baseline.project_docs.clone(),
            commit_log: baseline.commit_log.clone(),
        },
        repo_root: repo_root.to_string(),
        is_path_scan,
    };

    Ok((ctx, secret_findings))
}

/// Render findings and print output, handling format-specific side effects.
async fn render_and_output(
    format: &OutputFormat,
    findings: &[models::finding::Finding],
    fail_on: Option<Severity>,
) {
    use std::io::Write;

    let rendered = format.render(findings);
    print!("{rendered}");

    // Flush stdout so all findings appear before any stderr messages (summary,
    // error lines). Without this, CI environments block-buffer stdout and
    // interleave it with the immediately-flushed stderr output.
    let _ = std::io::stdout().flush();

    let env = Env::real();

    // Publish to external APIs where applicable (Bitbucket, Forgejo)
    if let Err(e) = format.publish(findings, fail_on, &env).await {
        eprintln!("Warning: failed to publish findings: {e}");
    }
}
