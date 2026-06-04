//! nitpik — AI-powered code review CLI.
//!
//! Entry point and error handling boundary. Uses `anyhow` for
//! ergonomic error propagation and user-facing messages.

mod cli;

use nitpik::agents;
use nitpik::audit;
use nitpik::cache;
use nitpik::config;
use nitpik::constants;
use nitpik::context;
use nitpik::diff;
use nitpik::env;
use nitpik::license;
use nitpik::models;
use nitpik::progress;
use nitpik::providers::ReviewProvider;
use nitpik::providers::rig::RigProvider;
use nitpik::review;
use nitpik::telemetry;
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
use models::Severity;
use progress::ProgressTracker;

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
        Command::Serve(args) => run_serve(args).await,
        Command::Version => run_version(),
    }
}

/// Run nitpik as a long-lived LSP or MCP server over stdio.
async fn run_serve(args: cli::args::ServeArgs) -> Result<()> {
    use cli::args::ServeTransport;
    match args.transport {
        ServeTransport::Lsp(a) => nitpik::lsp::serve(a.path).await,
        ServeTransport::Mcp(a) => nitpik::mcp::serve(a.path).await,
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
            if !license::is_valid_key_format(&key) {
                bail!("license key has invalid format (expected nkp_live_… or nkp_test_…)");
            }

            write_license_key_to_config(&key)?;
            // Drop any stale cache bound to a previous key.
            let _ = license::clear_cache();

            // Do an initial fetch so the user gets an immediate yes/no.
            let env = Env::real();
            let config = Config::load(None, &env).context("failed to load configuration")?;
            match license::verify_entitlement(&config, &env).await {
                Some(claims) => {
                    println!(
                        "  {} License activated. Plan: {}, type: {:?}.",
                        "✔".green().bold(),
                        claims.plan.bold(),
                        claims.kind,
                    );
                }
                None => {
                    println!(
                        "  {} {}",
                        "⚠".yellow().bold(),
                        "Key saved, but the initial verification failed. Run `nitpik license refresh` once the issue is resolved.".yellow(),
                    );
                }
            }
        }
        LicenseAction::Status => {
            let env = Env::real();
            let config = Config::load(None, &env).context("failed to load configuration")?;
            print_license_status(&config, &env).await;
        }
        LicenseAction::Deactivate => {
            remove_license_key_from_config()?;
            let _ = license::clear_cache();
            println!(
                "  {} License key removed and cached entitlement cleared.",
                "✔".green().bold(),
            );
        }
        LicenseAction::Refresh => {
            let _ = license::clear_cache();
            let env = Env::real();
            let config = Config::load(None, &env).context("failed to load configuration")?;
            match license::verify_entitlement(&config, &env).await {
                Some(claims) => {
                    println!(
                        "  {} Entitlement refreshed. Plan: {}, type: {:?}.",
                        "✔".green().bold(),
                        claims.plan.bold(),
                        claims.kind,
                    );
                }
                None => {
                    bail!("could not refresh the entitlement — see warnings above for the reason");
                }
            }
        }
    }

    Ok(())
}

/// Render the current license + entitlement state for the `status` subcommand.
async fn print_license_status(config: &Config, env: &Env) {
    use colored::Colorize;

    match config.license.key.as_ref() {
        Some(key) if license::is_valid_key_format(key) => {
            println!("  {}   {}…", "Key:".cyan(), &key[..key.len().min(12)]);
        }
        Some(_) => {
            println!(
                "  {} {}",
                "✖".red().bold(),
                "License key has an invalid format.".red(),
            );
            return;
        }
        None => {
            println!("  No license key configured.");
            println!("  Use `nitpik license activate <KEY>` to add one.");
            return;
        }
    }

    match license::verify_entitlement(config, env).await {
        Some(claims) => {
            println!("  {} {}", "Plan:".cyan(), claims.plan);
            println!("  {}  {:?}", "Type:".cyan(), claims.kind);
            println!(
                "  {} {}",
                "Token expires:".cyan(),
                format_unix_seconds(claims.expires_at),
            );
            println!("  {} {}", "Status:".cyan(), "valid".green());
        }
        None => {
            println!(
                "  {} {}",
                "Status:".cyan(),
                "not verified — see warnings above".yellow(),
            );
        }
    }
}

fn format_unix_seconds(epoch: i64) -> String {
    let days = epoch.div_euclid(86400);
    let rem = epoch.rem_euclid(86400);
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let days = days + 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 {
        y += 1;
    }
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02} UTC")
}

fn write_license_key_to_config(key: &str) -> Result<()> {
    let config_dir = dirs::config_dir()
        .map(|d| d.join(constants::CONFIG_DIR))
        .context("could not determine config directory")?;
    std::fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("config.toml");

    let mut content = if config_path.exists() {
        std::fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

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
    Ok(())
}

fn remove_license_key_from_config() -> Result<()> {
    let config_dir = dirs::config_dir()
        .map(|d| d.join(constants::CONFIG_DIR))
        .context("could not determine config directory")?;
    let config_path = config_dir.join("config.toml");
    if !config_path.exists() {
        return Ok(());
    }
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
    Ok(())
}
async fn run_review(args: cli::args::ReviewArgs, no_telemetry: bool) -> Result<()> {
    let input_mode = args.validate_input().map_err(|e| anyhow::anyhow!("{e}"))?;
    let repo_root = review::resolve_repo_root(&args.path).await?;
    let repo_root_path = Path::new(&repo_root);

    let env_real = Env::real();
    let config =
        Config::load(Some(repo_root_path), &env_real).context("failed to load configuration")?;
    let license_claims = verify_license(&config, &env_real).await;

    // Agentic policy: an explicit `--agent[=auto|on|off]` wins; otherwise
    // fall back to config (`[review.agentic] enabled` true → force on) and
    // finally to `auto` (honor each reviewer's own declaration).
    let agent_policy = match args.agent {
        Some(arg) => arg.into(),
        None if config.review.agentic.enabled => models::AgentPolicy::On,
        None => models::AgentPolicy::Auto,
    };
    let scan_secrets = args.scan_secrets || config.secrets.enabled;
    let scan_threats = args.scan_threats || config.threats.enabled;

    // Resolve audit log destination: CLI > env (already in config via
    // apply_env_vars) > [review].audit_log in TOML.
    let audit_path: Option<std::path::PathBuf> = args
        .audit_log
        .clone()
        .or_else(|| config.review.audit_log.clone());
    let audit_started_at = audit::now_unix_ms();
    let audit_started_instant = std::time::Instant::now();

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

    let is_path_scan = matches!(input_mode, models::InputMode::DirectPath(_));

    let show_threat_progress =
        !args.quiet && args.format == OutputFormat::Terminal && std::io::stderr().is_terminal();

    let options = review::ReviewOptions {
        profiles: args.profile.clone(),
        profile_dir: args.profile_dir.clone(),
        tags: args.tag.clone(),
        auto_mode: args.auto_mode,
        agent_policy,
        scan_secrets,
        scan_threats,
        secrets_rules: args.secrets_rules.clone(),
        secrets_severity: args.secrets_severity,
        threat_rules: args.threat_rules.clone(),
        no_project_docs: args.no_project_docs,
        no_commit_context: args.no_commit_context,
        exclude_doc: args.exclude_doc.clone(),
        rolling_summary: args.pr_summary || config.review.context.rolling_summary,
        no_cache: args.no_cache,
        no_prior_context: args.no_prior_context,
        max_prior_findings: args.max_prior_findings,
        verify: args.verify,
        multi_wave: args.multi_wave,
        max_concurrent: args.max_concurrent,
        max_turns: args.max_turns,
        max_tool_calls: args.max_tool_calls,
        timeout: if args.timeout == 0 {
            None
        } else {
            Some(std::time::Duration::from_secs(args.timeout))
        },
        audit_enabled: audit_path.is_some(),
        show_threat_progress,
    };

    // Construct the LLM provider once at the composition root and inject it
    // into both profile resolution and the review engine. Built as a
    // `Result` so the key-free paths below (`--debug-prompt`, heuristic
    // auto-selection) still work without an API key: they get `None`, while
    // an actual review unwraps it and surfaces the real construction error.
    let provider_result: std::result::Result<Arc<dyn ReviewProvider>, _> =
        RigProvider::new(config.provider.clone(), repo_root_path.to_path_buf())
            .map(|p| Arc::new(p) as Arc<dyn ReviewProvider>);

    let review::ResolvedAgents {
        agents: agent_defs,
        selection_tokens,
        selection_model,
    } = review::resolve_agents(
        provider_result.as_ref().ok().map(|p| p.as_ref()),
        &options,
        &config,
        diffs,
        repo_root_path,
    )
    .await?;

    let commit_log =
        review::build_commit_log(args.no_commit_context, &input_mode, repo_root_path).await;
    let pr_intent = context::pr_intent::detect_pr_intent(&env_real, !args.no_pr_intent);
    let mut baseline = context::build_baseline_context(
        repo_root_path,
        diffs,
        &config,
        args.no_project_docs,
        &args.exclude_doc,
        commit_log,
        pr_intent,
    )
    .await;
    // Prior review threads need an async forge fetch, so they're gathered
    // here rather than inside the (filesystem-only) baseline builder.
    baseline.prior_threads =
        context::pr_threads::gather_prior_threads(&env_real, args.pr_threads).await;

    // Debug-only: dump constructed prompts and exit without calling the LLM.
    #[cfg(debug_assertions)]
    if args.debug_prompt {
        use nitpik::diff::chunker;
        use nitpik::orchestrator::prompt::{build_prompt, build_system_addendum};

        let review_ctx = models::context::ReviewContext {
            diffs: diffs.to_vec(),
            baseline,
            repo_root: repo_root.clone(),
            is_path_scan,
        };

        // The addendum (PR intent/summary/prior threads/docs/commits) is
        // shared across every file × agent, so the orchestrator prepends it
        // once to each system prompt. Print it once here.
        let addendum = build_system_addendum(&review_ctx);
        if !addendum.is_empty() {
            println!("═══ shared system addendum ═══");
            println!("{addendum}");
        }

        for agent in &agent_defs {
            for d in diffs {
                if d.is_binary {
                    continue;
                }
                let chunks = chunker::chunk_diff(d, None);
                for chunk in chunks {
                    let user_prompt = build_prompt(
                        &chunk,
                        &review_ctx,
                        agent,
                        &agent_defs,
                        None,
                        agent_policy.resolve(agent.profile.agentic),
                    );
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

    let progress = setup_progress(
        &args,
        diffs,
        &agent_defs,
        &baseline,
        &license_claims,
        agent_policy != models::AgentPolicy::Off,
    );

    // `execute_review` owns the engine lifecycle, including progress
    // start/finish and the optional threat scan.
    // A real review needs the provider — surface the construction error now
    // (e.g. a missing API key) rather than silently producing no findings.
    let provider = provider_result.map_err(|e| anyhow::anyhow!("{e}"))?;
    // Keep a handle for the optional prior-feedback resolution pass, which
    // runs after the review but reuses the same provider.
    let resolve_provider = args.resolve_addressed.then(|| Arc::clone(&provider));
    let output = review::execute_review(
        provider,
        &config,
        &repo_root,
        diffs,
        is_path_scan,
        &agent_defs,
        baseline,
        &options,
        Arc::clone(&progress) as Arc<dyn progress::ProgressReporter>,
    )
    .await?;

    // Fold in the tokens spent on `auto` profile selection (an LLM call
    // made before the review proper) so the totals and per-model breakdown
    // account for every call this run made.
    let mut output = output;
    if selection_tokens.total() > 0 {
        output.tokens += selection_tokens;
        if let Some(model) = &selection_model {
            *output.tokens_by_model.entry(model.clone()).or_default() += selection_tokens;
        }
    }

    let findings = output.findings;

    // Prior-feedback retirement (opt-in): reply to + resolve nitpik threads
    // the latest push addressed. Best-effort; runs before publishing the new
    // review so the thread state aligns. Tokens fold into the run total.
    if let Some(provider) = resolve_provider
        && let Some(forge) = nitpik::forge::detect(&env_real)
    {
        let head_sha = env_real.var("GITHUB_SHA").ok();
        let outcome = review::resolve::resolve_addressed_threads(
            &provider,
            &output.resolved_model,
            forge.as_ref(),
            diffs,
            head_sha.as_deref(),
        )
        .await;
        if !args.quiet && !outcome.resolved.is_empty() {
            eprintln!(
                "Resolved {} addressed thread(s): {}",
                outcome.resolved.len(),
                outcome.resolved.join(", ")
            );
        }
        if outcome.tokens.total() > 0 {
            let model = output.resolved_model.clone();
            output.tokens += outcome.tokens;
            *output.tokens_by_model.entry(model).or_default() += outcome.tokens;
        }
    }

    if args.show_dropped && !args.quiet && !output.result.dropped.is_empty() {
        eprintln!(
            "\nDropped {} finding(s) by critic verify pass:",
            output.result.dropped.len()
        );
        for d in &output.result.dropped {
            eprintln!(
                "  - {}:{} {} — {}",
                d.finding.file, d.finding.line, d.finding.title, d.reason
            );
        }
    }

    let show_tokens = !args.quiet
        && !args.no_tokens
        && args.format == OutputFormat::Terminal
        && std::io::stderr().is_terminal();
    if show_tokens && output.tokens.total() > 0 {
        print_token_summary(output.tokens, &output.tokens_by_model);
    }

    // Write audit log artifact (opt-in via --audit-log / NITPIK_AUDIT_LOG / TOML).
    if let Some(ref path) = audit_path {
        let summary = audit::ConfigSummary {
            provider: config.provider.name.to_string(),
            model: output.resolved_model.clone(),
            agentic: agent_policy != models::AgentPolicy::Off,
            max_turns: args.max_turns,
            max_tool_calls: args.max_tool_calls,
            max_concurrent: args.max_concurrent,
            profiles: output.agent_profile_names.clone(),
            multi_wave: args.multi_wave,
            verify: args.verify,
            auto_mode: args.auto_mode,
            review_scope: diff::git::detect_branch(repo_root_path, &Env::real()).await,
            nitpik_version: constants::VERSION.to_string(),
            timeout_secs: args.timeout,
        };
        let document = audit::RunAudit {
            schema: 1,
            run_id: uuid::Uuid::new_v4().to_string(),
            started_at_unix_ms: audit_started_at,
            duration_ms: audit_started_instant.elapsed().as_millis() as u64,
            config: summary,
            tasks: output.result.task_audits.clone(),
            verify: output.result.verify_audit.clone(),
            findings: findings.clone(),
            failed_tasks: output.result.failed_tasks,
            tokens: output.tokens,
        };
        match document.write_to(path) {
            Ok(()) => {
                if !args.quiet {
                    eprintln!("Audit log written to {}", path.display());
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: failed to write audit log to {}: {e}",
                    path.display()
                );
            }
        }
    }

    let fail_on_severity: Option<Severity> = if args.no_fail {
        None
    } else {
        args.fail_on
            .or(config.review.fail_on)
            .or(Some(Severity::Error))
    };
    render_and_output(
        &args.format,
        &findings,
        fail_on_severity,
        args.request_changes,
        args.force_review,
    )
    .await;

    // Ensure the telemetry POST completes before the runtime shuts down.
    if let Some(h) = heartbeat {
        let _ = h.await;
    }

    determine_exit(
        &findings,
        fail_on_severity,
        &args.format,
        output.result.failed_tasks,
    )
}

/// Print a token usage summary to stderr.
///
/// When all tokens are attributed to a single model the output is a
/// one-liner. When multiple models contributed (because profile
/// overrides selected different models), the totals are followed by
/// a per-model breakdown.
fn print_token_summary(
    usage: nitpik::models::TokenUsage,
    by_model: &std::collections::BTreeMap<String, nitpik::models::TokenUsage>,
) {
    use colored::Colorize;
    use std::io::Write;

    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    let _ = writeln!(
        handle,
        "  {} {}",
        "▸".cyan().bold(),
        usage.format_summary().dimmed(),
    );

    if by_model.len() > 1 {
        for (model, u) in by_model {
            let _ = writeln!(
                handle,
                "    {} {} {}",
                "→".dimmed(),
                model.cyan(),
                u.format_summary().dimmed(),
            );
        }
    }

    let _ = writeln!(handle);
    let _ = handle.flush();
}

/// Resolve the active entitlement for the current review run.
///
/// Returns `None` (and prints stderr warnings) when no valid entitlement
/// can be established — the review continues in free-tier mode. Never
/// exits the process; the worst case is "unlicensed run."
async fn verify_license(config: &Config, env: &Env) -> Option<license::LicenseClaims> {
    license::verify_entitlement(config, env).await
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
    license_claims: &Option<license::LicenseClaims>,
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
    license_claims: &Option<license::LicenseClaims>,
    agentic: bool,
) -> Arc<ProgressTracker> {
    let is_interactive = args.format == OutputFormat::Terminal && std::io::stderr().is_terminal();
    let show_info = !args.quiet && is_interactive;
    let show_progress = !args.quiet && is_interactive;

    let file_names: Vec<String> = diffs.iter().map(|d| d.path().to_string()).collect();
    let agent_names: Vec<String> = agents.iter().map(|a| a.profile.name.clone()).collect();
    // Tool-log pane is hidden when --agent is off (no tools can fire).
    // Ring-buffer cap matches the plan: max(8, max_concurrent).
    let progress = Arc::new(
        ProgressTracker::new(&file_names, &agent_names, show_progress)
            .with_tool_log(agentic)
            .with_log_cap(std::cmp::max(8, args.max_concurrent)),
    );

    if show_info {
        cli::print_banner(license_claims.as_ref());

        // Offline tokens have a real wall-clock expiry users must manage —
        // surface a soft warning when one is approaching its end. Online
        // entitlements refresh automatically so we stay quiet there.
        if let Some(claims) = license_claims
            && claims.kind == license::TokenKind::Offline
        {
            use colored::Colorize;
            use std::io::Write;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let days_remaining = (claims.expires_at - now) / 86_400;
            if (0..=14).contains(&days_remaining) {
                let stderr = std::io::stderr();
                let mut handle = stderr.lock();
                let _ = writeln!(
                        handle,
                        "  {} {}",
                        "⚠".yellow().bold(),
                        format!(
                            "Offline token expires in {days_remaining} day(s) — download a fresh one at https://nitpik.dev/account"
                        )
                        .yellow(),
                    );
                let _ = writeln!(handle);
                let _ = handle.flush();
            }
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

/// Render findings and print output, handling format-specific side effects.
async fn render_and_output(
    format: &OutputFormat,
    findings: &[models::finding::Finding],
    fail_on: Option<Severity>,
    request_changes: Option<Severity>,
    force_review: bool,
) {
    use std::io::Write;

    let rendered = format.render(findings);
    print!("{rendered}");

    // Flush stdout so all findings appear before any stderr messages (summary,
    // error lines). Without this, CI environments block-buffer stdout and
    // interleave it with the immediately-flushed stderr output.
    let _ = std::io::stdout().flush();

    let env = Env::real();

    // Choose the PR-review action (advisory comment vs. blocking
    // changes-requested); only the GitHub PR-review publisher consults it.
    let review_event = nitpik::forge::review_event_for(findings, request_changes);

    // Publish to external APIs where applicable (Bitbucket, Forgejo, GitHub PR review)
    if let Err(e) = format
        .publish(findings, fail_on, review_event, force_review, &env)
        .await
    {
        eprintln!("Warning: failed to publish findings: {e}");
    }
}
