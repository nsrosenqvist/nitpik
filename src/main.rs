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
use nitpik::output;
use nitpik::progress;
use nitpik::providers;
use nitpik::security;
use nitpik::telemetry;
use nitpik::update;

use std::path::Path;
use std::process;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;

use cli::args::{CacheAction, Cli, Command, LicenseAction, OutputFormat, UpdateArgs};
use config::Config;
use env::Env;
use models::{Severity, DEFAULT_PROFILE};
use progress::ProgressTracker;
use providers::rig::RigProvider;
use providers::ReviewProvider;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("Error: {err:#}");
        process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    let no_telemetry = cli.no_telemetry;

    match cli.command {
        Command::Review(args) => run_review(args, no_telemetry).await,
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

    println!(
        "{} {}",
        "nitpik".bold(),
        constants::VERSION.green().bold()
    );
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
        println!(
            "  {}  {}",
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
            println!("         {}  {} chars", "prompt:".cyan(), agent.system_prompt.len());
            Ok(())
        }
        Err(e) => {
            use colored::Colorize;
            bail!("{} {}", "✖".red().bold(), format!("Invalid profile: {e}").red());
        }
    }
}

/// Manage the result cache.
async fn run_cache(action: CacheAction) -> Result<()> {
    let engine = cache::CacheEngine::new(true);

    match action {
        CacheAction::Clear => {
            let stats = engine.clear().context("failed to clear cache")?;
            println!(
                "Cleared {} cached entry/entries ({}).",
                stats.entries,
                stats.human_size(),
            );
        }
        CacheAction::Stats => {
            let stats = engine.stats().context("failed to read cache stats")?;
            println!("Cache entries: {}", stats.entries);
            println!("Cache size:    {}", stats.human_size());
        }
        CacheAction::Path => {
            match engine.path() {
                Some(p) => println!("{}", p.display()),
                None => bail!("cache directory could not be determined"),
            }
        }
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
            let claims = license::verify_license_key(&key)
                .context("invalid license key")?;
            let expiry = license::check_expiry(&claims)
                .context("failed to check license expiry")?;

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
            let config = Config::load(None, &Env::real()).context("failed to load configuration")?;

            match config.license.key {
                Some(ref key) => {
                    match license::verify_license_key(key) {
                        Ok(claims) => {
                            let expiry = license::check_expiry(&claims)
                                .context("failed to check expiry")?;
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
                    }
                }
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

            println!(
                "  {} License key removed.",
                "✔".green().bold(),
            );
        }
    }

    Ok(())
}
async fn run_review(args: cli::args::ReviewArgs, no_telemetry: bool) -> Result<()> {
    // Validate input mode
    let input_mode = args
        .validate_input()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Resolve repo / working directory from --path (default: cwd)
    let base_dir = std::fs::canonicalize(&args.path)
        .with_context(|| format!("--path directory not found: {}", args.path.display()))?;
    let repo_root = match diff::git::find_repo_root(&base_dir).await {
        Ok(root) => root,
        Err(_) => base_dir.display().to_string(),
    };
    let repo_root_path = Path::new(&repo_root);

    // Load config with layering
    let config = Config::load(Some(repo_root_path), &Env::real())
        .context("failed to load configuration")?;

    // Verify license key (if present)
    let license_claims = if let Some(ref key) = config.license.key {
        match license::verify_license_key(key) {
            Ok(claims) => {
                match license::check_expiry(&claims) {
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
                    Ok(license::ExpiryStatus::ExpiringSoon { days }) => {
                        Some((claims, Some(days)))
                    }
                    Ok(license::ExpiryStatus::Valid) => Some((claims, None)),
                    Err(e) => {
                        eprintln!("Warning: could not check license expiry: {e}");
                        Some((claims, None))
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: invalid license key: {e}");
                None
            }
        }
    } else {
        None
    };

    // Apply config defaults where CLI didn't override
    let use_agent = args.agent || config.review.agentic.enabled;
    let scan_secrets = args.scan_secrets || config.secrets.enabled;
    let no_cache = args.no_cache;

    // Get diffs
    let diffs = diff::get_diffs(&input_mode, repo_root_path)
        .await
        .context("failed to get diffs")?;

    if diffs.is_empty() {
        eprintln!("No changes to review.");
        return Ok(());
    }

    // Build baseline context
    let baseline = context::build_baseline_context(repo_root_path, &diffs, &config).await;

    // Resolve agent profiles
    let agent_defs = resolve_agents(&args, &config, &diffs).await?;

    // Fire anonymous telemetry heartbeat (non-blocking, fails silently)
    let telemetry_enabled = config.telemetry.enabled && !no_telemetry;
    if telemetry_enabled {
        use models::diff::DiffLineType;
        let diff_lines: usize = diffs
            .iter()
            .flat_map(|d| d.hunks.iter())
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.line_type == DiffLineType::Added || l.line_type == DiffLineType::Removed)
            .count();
        let licensed = license_claims.is_some();
        let payload = telemetry::HeartbeatPayload::from_review(
            diffs.len(),
            diff_lines,
            agent_defs.len(),
            licensed,
        );
        let handle = telemetry::send_heartbeat(payload);
        if telemetry::is_debug() {
            let _ = handle.await;
        }
    }

    // Determine whether to show progress
    // Disable progress for non-terminal formats (CI output) or when explicitly disabled
    let show_progress = !args.no_progress
        && args.format == OutputFormat::Terminal
        && atty::is(atty::Stream::Stderr);

    // Build progress tracker
    let file_names: Vec<String> = diffs.iter().map(|d| d.path().to_string()).collect();
    let agent_names: Vec<String> = agent_defs.iter().map(|a| a.profile.name.clone()).collect();
    let progress = std::sync::Arc::new(ProgressTracker::new(&file_names, &agent_names, show_progress));
    if show_progress {
        let claims_ref = license_claims.as_ref().map(|(c, _)| c);
        cli::print_banner(claims_ref);

        // Show expiry warning if license is expiring soon
        if let Some((_, Some(days))) = &license_claims {
            use colored::Colorize;
            use std::io::Write;
            let stderr = std::io::stderr();
            let mut handle = stderr.lock();
            if *days <= 13 {
                let _ = writeln!(
                    handle,
                    "  {} {}",
                    "⚠".yellow().bold(),
                    format!("License expires in {days} day(s) — renew at https://nitpik.dev").yellow(),
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
    }
    progress.start();

    // Secret scanning and context construction
    let is_path_scan = matches!(input_mode, models::InputMode::DirectPath(_));
    let (review_context, secret_findings) = build_review_context(
        &args, &config, &diffs, baseline, &repo_root, scan_secrets, is_path_scan,
    )?;

    // Set up provider
    let provider: Arc<dyn ReviewProvider> = Arc::new(
        RigProvider::new(config.provider.clone(), repo_root_path.to_path_buf())
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    );

    // Detect branch/PR scope for sidecar isolation
    let review_scope = diff::git::detect_branch(repo_root_path, &Env::real()).await;

    // Run orchestrator
    let cache = cache::CacheEngine::new(!no_cache);

    // Clean up stale sidecar files (>30 days) before the review starts
    let stale_age = std::time::Duration::from_secs(30 * 24 * 60 * 60);
    let _removed = cache.cleanup_stale(stale_age);

    let orchestrator = orchestrator::ReviewOrchestrator::new(
        Arc::clone(&provider),
        &config,
        cache,
        Arc::clone(&progress),
        args.no_prior_context,
        args.max_prior_findings,
        review_scope,
    );

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

    let mut findings = review_result.findings;
    let failed_tasks = review_result.failed_tasks;

    // Add secret scanner findings
    findings.extend(secret_findings);

    // Sort findings by severity (errors first), then file, then line
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });

    // Finish progress display
    progress.finish(findings.len());

    // Resolve fail-on threshold (CLI flag takes priority over config)
    let fail_on_severity: Option<Severity> = args.fail_on.or(config.review.fail_on);

    // Render and print output
    render_and_output(&args.format, &findings, fail_on_severity).await;

    // Exit with non-zero code if findings exceed fail_on threshold
    if let Some(threshold) = fail_on_severity {
        let failing: Vec<_> = findings.iter().filter(|f| f.severity >= threshold).collect();
        if !failing.is_empty() {
            let summary = models::finding::Summary::from_findings(&findings);
            eprintln!(
                "\nReview complete: {} error(s), {} warning(s), {} info — failing on {threshold}+",
                summary.errors, summary.warnings, summary.info,
            );
            bail!(
                "found {} finding(s) at or above {threshold} threshold",
                failing.len(),
            );
        }
    }

    // Fail if any review tasks could not complete
    if failed_tasks > 0 {
        bail!(
            "{failed_tasks} review task(s) failed after retries — results are incomplete",
        );
    }

    Ok(())
}

/// Resolve agent profiles from CLI args and config.
async fn resolve_agents(
    args: &cli::args::ReviewArgs,
    config: &Config,
    diffs: &[models::FileDiff],
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
        agents::auto::auto_select_profiles(diffs)
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
fn build_review_context(
    args: &cli::args::ReviewArgs,
    config: &Config,
    diffs: &[models::FileDiff],
    baseline: models::BaselineContext,
    repo_root: &str,
    scan_secrets: bool,
    is_path_scan: bool,
) -> Result<(models::ReviewContext, Vec<models::finding::Finding>)> {
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
        .or_else(|| config_rules_path.as_deref());
    if let Some(rules_path) = rules_path {
        let extra = security::rules::load_rules_from_file(rules_path)
            .map_err(|e| anyhow::anyhow!("failed to load secret rules: {e}"))?;
        rules.extend(extra);
    }

    // Scan and redact baseline file contents
    let mut secret_findings = Vec::new();
    let mut redacted_contents = indexmap::IndexMap::new();
    for (path, content) in &baseline.file_contents {
        let (redacted, findings) = security::scan_and_redact(content, path, &rules);
        secret_findings.extend(findings);
        redacted_contents.insert(path.clone(), redacted);
    }

    let ctx = models::ReviewContext {
        diffs: diffs.to_vec(),
        baseline: models::BaselineContext {
            file_contents: redacted_contents,
            project_docs: baseline.project_docs.clone(),
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
    let rendered = format.render(findings);
    print!("{rendered}");

    let env = Env::real();

    // Bitbucket: also post to API if env vars are set
    if *format == OutputFormat::Bitbucket && env.is_set("BITBUCKET_WORKSPACE") {
        if let Err(e) = output::bitbucket::post_to_bitbucket(findings, fail_on, &env).await {
            eprintln!("Warning: failed to post to Bitbucket: {e}");
        }
    }

    // Forgejo/Gitea: post review via API if env vars are set
    if *format == OutputFormat::Forgejo && env.is_set("CI_FORGE_URL") {
        if let Err(e) = output::forgejo::post_to_forgejo(findings, &env).await {
            eprintln!("Warning: failed to post to Forgejo: {e}");
        }
    }
}
