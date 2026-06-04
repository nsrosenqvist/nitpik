//! Shared review engine entrypoint.
//!
//! [`execute_review`] is the single code path that turns a resolved
//! [`InputMode`] + [`Config`] into findings. The CLI (`nitpik review`),
//! the LSP server (`nitpik serve lsp`), and the MCP server
//! (`nitpik serve mcp`) all call it, so behaviour stays identical across
//! surfaces.
//!
//! This module owns the *engine* concerns only — diff parsing, baseline
//! context, agent resolution, orchestration, secret/threat scanning, and
//! finding aggregation. Presentation concerns (terminal progress, token
//! summaries, output rendering, exit codes, telemetry, audit-log writing)
//! stay with the caller, which is why the function returns a fully-owned
//! [`ReviewOutput`] rather than printing anything itself.

pub mod summary;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::agents;
use crate::cache;
use crate::config::Config;
use crate::diff;
use crate::env::Env;
use crate::models::{self, AgentDefinition, DEFAULT_PROFILE, Severity};
use crate::orchestrator::{self, ReviewOrchestrator, ReviewResult};
use crate::progress::{ProgressReporter, TaskStatus};
use crate::providers::ReviewProvider;
use crate::security;
use crate::threat;

/// Knobs controlling a single review run.
///
/// Mirrors the engine-relevant subset of the CLI's `ReviewArgs`, decoupled
/// from clap so non-CLI callers (LSP/MCP servers) can drive reviews without
/// constructing argument structs.
#[derive(Debug, Clone)]
pub struct ReviewOptions {
    // --- Profile selection ---
    /// Profiles to run: built-in names, file paths, or `"auto"`.
    pub profiles: Vec<String>,
    /// Directory to resolve bare profile names from.
    pub profile_dir: Option<PathBuf>,
    /// Additional profiles selected by tag.
    pub tags: Vec<String>,
    /// Strategy for `auto` profile selection (only meaningful with `"auto"`).
    pub auto_mode: Option<agents::auto::AutoMode>,

    // --- Engine toggles ---
    /// Enable agentic context gathering (tools for the LLM).
    pub use_agent: bool,
    /// Scan for and redact secrets before LLM calls.
    pub scan_secrets: bool,
    /// Scan diffs for threat patterns (then LLM-triage matches).
    pub scan_threats: bool,
    /// Additional gitleaks-format secret rules file.
    pub secrets_rules: Option<PathBuf>,
    /// Severity assigned to detected secrets (falls back to config).
    pub secrets_severity: Option<Severity>,
    /// Additional threat rules file.
    pub threat_rules: Option<PathBuf>,

    // --- Context ---
    /// Skip auto-detected project documentation files.
    pub no_project_docs: bool,
    /// Skip injecting commit summaries into the prompt.
    pub no_commit_context: bool,
    /// Project documentation files to exclude by name.
    pub exclude_doc: Vec<String>,
    /// Generate/refresh a rolling functional PR summary (one extra LLM call
    /// per run) and inject it into every reviewer's context. Persisted per
    /// branch so it accumulates across pushes. Off by default.
    pub rolling_summary: bool,

    // --- Performance / cache ---
    /// Disable result caching (force fresh LLM calls).
    pub no_cache: bool,
    /// Disable injection of prior findings on cache invalidation.
    pub no_prior_context: bool,
    /// Cap prior findings injected into the prompt.
    pub max_prior_findings: Option<usize>,
    /// Run a critic verify pass to drop probable false positives.
    pub verify: bool,
    /// Run reviewers in waves (`wave: 2` profiles run after wave 1).
    pub multi_wave: bool,
    /// Max concurrent LLM calls.
    pub max_concurrent: usize,
    /// Max agentic loop turns per file×agent.
    pub max_turns: usize,
    /// Max tool invocations per file×agent.
    pub max_tool_calls: usize,
    /// Per-attempt timeout; `None` disables.
    pub timeout: Option<Duration>,

    // --- Misc ---
    /// Collect per-task audit records (needed only when the caller writes
    /// an audit-log artifact).
    pub audit_enabled: bool,
    /// Print transient threat-scanner status lines to stderr. The CLI sets
    /// this for interactive terminal runs; servers leave it `false`.
    pub show_threat_progress: bool,
}

impl Default for ReviewOptions {
    fn default() -> Self {
        Self {
            profiles: vec![DEFAULT_PROFILE.to_string()],
            profile_dir: None,
            tags: Vec::new(),
            auto_mode: None,
            use_agent: false,
            scan_secrets: false,
            scan_threats: false,
            secrets_rules: None,
            secrets_severity: None,
            threat_rules: None,
            no_project_docs: false,
            no_commit_context: false,
            exclude_doc: Vec::new(),
            rolling_summary: false,
            no_cache: false,
            no_prior_context: false,
            max_prior_findings: None,
            verify: false,
            multi_wave: false,
            max_concurrent: 5,
            max_turns: 10,
            max_tool_calls: 10,
            timeout: None,
            audit_enabled: false,
            show_threat_progress: false,
        }
    }
}

/// Result of a review run, fully owned (borrows nothing from the diff).
pub struct ReviewOutput {
    /// Merged, sorted findings (review + secrets + threats).
    pub findings: Vec<models::finding::Finding>,
    /// Total token usage including threat triage.
    pub tokens: models::TokenUsage,
    /// Per-model token breakdown including threat triage.
    pub tokens_by_model: BTreeMap<String, models::TokenUsage>,
    /// Raw orchestrator result — carries `dropped`, `task_audits`,
    /// `verify_audit`, and `failed_tasks` for callers that write audit logs
    /// or compute exit codes.
    pub result: ReviewResult,
    /// Resolved profile names that ran (for audit summaries).
    pub agent_profile_names: Vec<String>,
    /// The provider's resolved model identifier (for audit summaries).
    pub resolved_model: String,
}

/// A [`ProgressReporter`] that does nothing — for headless servers (LSP/MCP)
/// that have no live terminal display.
pub struct NoopProgress;

impl ProgressReporter for NoopProgress {
    fn update(&self, _file: &str, _agent: &str, _status: TaskStatus) {}
    fn start(&self) {}
    fn finish(&self) {}
}

/// Ensure the process holds an active subscription entitlement, as required
/// for editor/agent (LSP/MCP) reviews.
///
/// Unlike the CLI — which degrades to free-tier on a missing entitlement —
/// the editor and agent surfaces are gated. Returns a user-facing message
/// when no active entitlement is present.
pub async fn require_entitlement(config: &Config, env: &Env) -> std::result::Result<(), String> {
    if crate::license::verify_entitlement(config, env)
        .await
        .is_some()
    {
        Ok(())
    } else {
        Err(
            "nitpik: an active subscription is required for editor and agent \
             integrations. Sign in and manage your subscription at \
             https://nitpik.dev/account."
                .to_string(),
        )
    }
}

/// Resolve the repository root for a `--path` argument.
///
/// Falls back to the provided directory when it is not inside a git repo
/// (e.g. `--scan` of a loose directory).
pub async fn resolve_repo_root(path: &Path) -> Result<String> {
    let base_dir = std::fs::canonicalize(path)
        .with_context(|| format!("--path directory not found: {}", path.display()))?;
    match diff::git::find_repo_root(&base_dir).await {
        Ok(root) => Ok(root),
        Err(_) => Ok(base_dir.display().to_string()),
    }
}

/// Run a review end-to-end and return owned findings + metadata.
///
/// The caller resolves and passes in the engine inputs — `diffs` (keeping the
/// backing diff source alive for the borrow), `agent_defs`, and `baseline` —
/// via the [`resolve_agents`] / [`diff::get_diff_source`] /
/// [`build_baseline_context`](crate::context::build_baseline_context) helpers.
/// This lets the caller build its own progress UI and telemetry from the same
/// inputs without the engine resolving profiles twice (auto-mode selection may
/// make an LLM call). `repo_root` must already be resolved (see
/// [`resolve_repo_root`]) and `config` loaded. Entitlement gating, telemetry,
/// and all output are the caller's job.
#[allow(clippy::too_many_arguments)]
pub async fn execute_review<'a>(
    provider: Arc<dyn ReviewProvider>,
    config: &Config,
    repo_root: &str,
    diffs: &[models::FileDiff<'a>],
    is_path_scan: bool,
    agent_defs: &[AgentDefinition],
    baseline: models::BaselineContext,
    options: &ReviewOptions,
    progress: Arc<dyn ProgressReporter>,
) -> Result<ReviewOutput> {
    let repo_root_path = Path::new(repo_root);

    let agent_profile_names: Vec<String> =
        agent_defs.iter().map(|a| a.profile.name.clone()).collect();

    let orchestrator = create_orchestrator(
        Arc::clone(&provider),
        config,
        repo_root_path,
        options.no_cache,
        Arc::clone(&progress),
        options.no_prior_context,
        options.max_prior_findings,
        options.verify,
        options.multi_wave,
        options.audit_enabled,
        options.timeout,
    )
    .await;

    // Rolling PR summary (Phase C, gap c): an optional extra LLM call whose
    // result is injected into every reviewer's baseline context and
    // persisted per branch so it accumulates across pushes. Fail-open — a
    // summary failure never blocks the review.
    let mut baseline = baseline;
    let mut summary_tokens = models::TokenUsage::default();
    if options.rolling_summary {
        let summary_model = config.provider.model_for(crate::config::ModelTask::Summary);
        summary_tokens = refresh_pr_summary(
            provider.as_ref(),
            summary_model,
            repo_root_path,
            diffs,
            &mut baseline,
        )
        .await;
    }

    let (review_context, secret_findings) =
        build_review_context(options, config, diffs, baseline, repo_root, is_path_scan)?;

    progress.start();
    let review_result = orchestrator
        .run(
            &review_context,
            agent_defs,
            options.max_concurrent,
            options.use_agent,
            options.max_turns,
            options.max_tool_calls,
        )
        .await
        .context("review failed")?;
    progress.finish();

    // Threat scanning: fast pattern scan, then optional LLM triage (fail-open).
    let (threat_findings, triage_tokens) = if options.scan_threats {
        run_threat_scan(config, options, &review_context, provider.as_ref()).await
    } else {
        (Vec::new(), models::TokenUsage::default())
    };

    let mut findings = review_result.findings.clone();
    findings.extend(secret_findings);
    findings.extend(threat_findings);
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });

    let total_tokens = review_result.tokens + triage_tokens + summary_tokens;
    let mut tokens_by_model = review_result.tokens_by_model.clone();
    // Attribute each auxiliary call's tokens to the model that actually ran
    // it — they may differ from the review model under per-task overrides.
    if triage_tokens.total() > 0 {
        let model = config
            .provider
            .model_for(crate::config::ModelTask::Triage)
            .to_string();
        *tokens_by_model.entry(model).or_default() += triage_tokens;
    }
    if summary_tokens.total() > 0 {
        let model = config
            .provider
            .model_for(crate::config::ModelTask::Summary)
            .to_string();
        *tokens_by_model.entry(model).or_default() += summary_tokens;
    }

    Ok(ReviewOutput {
        findings,
        tokens: total_tokens,
        tokens_by_model,
        resolved_model: config.provider.resolved_model().to_string(),
        agent_profile_names,
        result: review_result,
    })
}

/// Fetch the commit log for baseline context, if applicable.
///
/// Exposed as a building block for callers (e.g. the CLI's debug
/// prompt-dump) that assemble baseline context outside [`execute_review`].
pub async fn build_commit_log(
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

/// Build the cache engine and review orchestrator around an injected provider.
#[allow(clippy::too_many_arguments)]
async fn create_orchestrator(
    provider: Arc<dyn ReviewProvider>,
    config: &Config,
    repo_root_path: &Path,
    no_cache: bool,
    progress: Arc<dyn ProgressReporter>,
    no_prior_context: bool,
    max_prior_findings: Option<usize>,
    verify: bool,
    multi_wave: bool,
    audit_enabled: bool,
    timeout: Option<Duration>,
) -> ReviewOrchestrator {
    let review_scope = diff::git::detect_branch(repo_root_path, &Env::real()).await;
    let cache = cache::CacheEngine::new(!no_cache);
    let stale_age = Duration::from_secs(30 * 24 * 60 * 60);
    let _removed = cache.cleanup_stale(stale_age).await;

    orchestrator::ReviewOrchestrator::new(
        provider,
        config,
        cache,
        progress,
        no_prior_context,
        max_prior_findings,
        review_scope,
        verify,
        multi_wave,
        audit_enabled,
        timeout,
    )
}

/// Generate/refresh the rolling PR summary and write it into `baseline`.
///
/// Loads any prior summary for this branch, asks the provider for a
/// refreshed one (folding in the prior), persists it, and sets
/// `baseline.pr_summary`. Fail-open at every step: on an empty result or a
/// provider error it falls back to the prior summary (or none) and returns
/// zero tokens, so the rest of the review proceeds unaffected. Returns the
/// tokens the summary call consumed (for accounting).
async fn refresh_pr_summary(
    provider: &dyn ReviewProvider,
    model: &str,
    repo_root_path: &Path,
    diffs: &[models::FileDiff<'_>],
    baseline: &mut models::BaselineContext,
) -> models::TokenUsage {
    let store = cache::store::FileStore::new();
    let scope = diff::git::detect_branch(repo_root_path, &Env::real()).await;
    let prior = store.get_summary(&scope).await;

    let system_prompt = summary::summary_system_prompt();
    let user_prompt = summary::build_summary_user_prompt(diffs, prior.as_deref());

    match provider
        .summarize(model, &system_prompt, &user_prompt)
        .await
    {
        Ok(outcome) if !outcome.summary.trim().is_empty() => {
            store.put_summary(&scope, &outcome.summary).await;
            baseline.pr_summary = Some(outcome.summary);
            outcome.tokens
        }
        Ok(_) => {
            // Empty result (e.g. a provider without summary support) — keep
            // whatever prior summary we had.
            baseline.pr_summary = prior;
            models::TokenUsage::default()
        }
        Err(e) => {
            eprintln!("Warning: PR summary generation failed ({e}); using prior summary if any.");
            baseline.pr_summary = prior;
            models::TokenUsage::default()
        }
    }
}

/// Result of [`resolve_agents`]: the agents to run plus the tokens spent
/// resolving them.
///
/// `auto` profile selection can make an LLM (triage) call before the review
/// proper begins; its cost is surfaced here so callers fold it into the
/// run's total and per-model token accounting rather than dropping it.
pub struct ResolvedAgents {
    /// The resolved agent definitions to run.
    pub agents: Vec<AgentDefinition>,
    /// Tokens consumed by `auto` profile selection (zero when no LLM call
    /// was made, e.g. heuristic-only selection or a non-`auto` profile set).
    pub selection_tokens: models::TokenUsage,
    /// The model that ran the selection call, when it spent tokens — lets
    /// the caller attribute the cost to the right model.
    pub selection_model: Option<String>,
}

/// Resolve the set of agent profiles to run for this review.
///
/// `provider` is injected for `auto` profile selection (the only step here
/// that may call an LLM). Pass `None` to force heuristic-only selection —
/// e.g. when no provider could be constructed — keeping key-free paths
/// (like the CLI's `--debug-prompt`) working.
///
/// Exposed as a building block for callers that need the resolved profile
/// set without running a review.
pub async fn resolve_agents(
    provider: Option<&dyn ReviewProvider>,
    options: &ReviewOptions,
    config: &Config,
    diffs: &[models::FileDiff<'_>],
    repo_root_path: &Path,
) -> Result<ResolvedAgents> {
    let profile_names = if options.profiles == vec![DEFAULT_PROFILE.to_string()] {
        // CLI default — check config for overrides
        if !config.review.default_profiles.is_empty() {
            config.review.default_profiles.clone()
        } else {
            options.profiles.clone()
        }
    } else {
        options.profiles.clone()
    };

    let used_auto = profile_names.iter().any(|p| p == "auto");
    if !used_auto && options.auto_mode.is_some() {
        eprintln!("warning: auto-mode is ignored because the profile set does not include 'auto'");
    }
    let (profiles, selection_tokens) = if used_auto {
        select_auto_profiles(provider, options, config, diffs, repo_root_path).await
    } else {
        (profile_names, models::TokenUsage::default())
    };

    let mut agent_defs = agents::resolve_profiles(&profiles, options.profile_dir.as_deref())
        .await
        .context("failed to resolve agent profiles")?;

    // Auto mode: splice in any profile that opted into `always_include`.
    if used_auto {
        let always_on = agents::list_always_include_profiles(options.profile_dir.as_deref())
            .await
            .context("failed to load always-include profiles")?;
        let existing: std::collections::HashSet<String> =
            agent_defs.iter().map(|a| a.profile.name.clone()).collect();
        for agent in always_on {
            if !existing.contains(&agent.profile.name) {
                agent_defs.push(agent);
            }
        }
    }

    // Tags: add any profiles matching the requested tags (deduped by name).
    if !options.tags.is_empty() {
        let by_tag =
            agents::resolve_profiles_by_tags(&options.tags, options.profile_dir.as_deref())
                .await
                .context("failed to resolve profiles by tag")?;
        let existing_names: std::collections::HashSet<String> =
            agent_defs.iter().map(|a| a.profile.name.clone()).collect();
        for agent in by_tag {
            if !existing_names.contains(&agent.profile.name) {
                agent_defs.push(agent);
            }
        }
    }

    let selection_model = if selection_tokens.total() > 0 {
        Some(
            config
                .provider
                .model_for(crate::config::ModelTask::Triage)
                .to_string(),
        )
    } else {
        None
    };

    Ok(ResolvedAgents {
        agents: agent_defs,
        selection_tokens,
        selection_model,
    })
}

/// Pick reviewer profiles for `auto`, honoring the auto-mode strategy.
///
/// The `provider` is injected by the caller (`None` when none could be
/// constructed, e.g. no API key — which keeps key-free paths like
/// `--debug-prompt` working). Returns the chosen profiles plus the tokens
/// the selection triage call consumed (zero when no LLM call was made).
/// Fails open to the heuristic result whenever an LLM call can't run or
/// errors.
async fn select_auto_profiles(
    provider: Option<&dyn ReviewProvider>,
    options: &ReviewOptions,
    config: &Config,
    diffs: &[models::FileDiff<'_>],
    repo_root_path: &Path,
) -> (Vec<String>, models::TokenUsage) {
    use agents::auto::AutoMode;
    let no_tokens = models::TokenUsage::default();
    let (heuristic, confidence) =
        agents::auto::auto_select_profiles_with_confidence(diffs, repo_root_path);

    let need_llm = match options.auto_mode.unwrap_or(AutoMode::Hybrid) {
        AutoMode::Heuristic => false,
        AutoMode::Llm => true,
        AutoMode::Hybrid => confidence == agents::auto::HeuristicConfidence::Low,
    };
    if !need_llm {
        return (heuristic, no_tokens);
    }

    let triage_agent = match agents::builtin::get_builtin("triage") {
        Some(a) => a,
        None => return (heuristic, no_tokens),
    };
    let Some(provider) = provider else {
        eprintln!("Warning: no provider available for auto-selection; using heuristic profiles.");
        return (heuristic, no_tokens);
    };

    let summary = agents::auto::build_triage_summary(diffs);
    let triage_model = config.provider.model_for(crate::config::ModelTask::Triage);
    auto_triage_select(
        provider,
        triage_model,
        &triage_agent.system_prompt,
        &summary,
        heuristic,
    )
    .await
}

/// Run the auto-selection triage call and interpret its result.
///
/// Extracted from [`select_auto_profiles`] so the provider-dependent logic
/// is unit-testable with a mock: this owns the token capture and the
/// verdict→profiles mapping, while the caller owns the (key-requiring,
/// untestable) provider construction. Fails open to `heuristic` on a
/// provider error, but still reports the tokens a successful call spent —
/// even when its verdicts turn out unusable and we fall back to heuristics.
async fn auto_triage_select(
    provider: &dyn ReviewProvider,
    triage_model: &str,
    triage_system_prompt: &str,
    triage_summary: &str,
    heuristic: Vec<String>,
) -> (Vec<String>, models::TokenUsage) {
    match provider
        .triage(triage_model, triage_system_prompt, triage_summary)
        .await
    {
        Ok(outcome) => {
            let mut picked = agents::auto::parse_triage_profiles(&outcome.verdicts);
            if heuristic.iter().any(|p| p == "architect")
                && !picked.iter().any(|p| p == "architect")
            {
                picked.push("architect".to_string());
            }
            let profiles = if picked.is_empty() { heuristic } else { picked };
            (profiles, outcome.tokens)
        }
        Err(e) => {
            eprintln!("Warning: triage call failed ({e}); using heuristic profiles.");
            (heuristic, models::TokenUsage::default())
        }
    }
}

/// Build review context, optionally scanning and redacting secrets.
fn build_review_context<'a>(
    options: &ReviewOptions,
    config: &Config,
    diffs: &[models::FileDiff<'a>],
    baseline: models::BaselineContext,
    repo_root: &str,
    is_path_scan: bool,
) -> Result<(
    models::context::ReviewContext<'a>,
    Vec<models::finding::Finding>,
)> {
    if !options.scan_secrets {
        let ctx = models::context::ReviewContext {
            diffs: diffs.to_vec(),
            baseline,
            repo_root: repo_root.to_string(),
            is_path_scan,
        };
        return Ok((ctx, Vec::new()));
    }

    let mut rules = security::rules::default_rules();

    let config_rules_path: Option<PathBuf> =
        config.secrets.additional_rules.as_ref().map(PathBuf::from);
    let rules_path = options
        .secrets_rules
        .as_deref()
        .or(config_rules_path.as_deref());
    if let Some(rules_path) = rules_path {
        let extra = security::rules::load_rules_from_file(rules_path)
            .map_err(|e| anyhow::anyhow!("failed to load secret rules: {e}"))?;
        rules.extend(extra);
    }

    let secrets_severity = options.secrets_severity.unwrap_or(config.secrets.severity);

    let mut secret_findings = Vec::new();
    let mut redacted_contents = indexmap::IndexMap::new();
    for (path, content) in &baseline.file_contents {
        let (redacted, findings) =
            security::scan_and_redact(content, path, &rules, secrets_severity);
        secret_findings.extend(findings);
        redacted_contents.insert(path.clone(), redacted);
    }

    let ctx = models::context::ReviewContext {
        diffs: diffs.to_vec(),
        baseline: models::BaselineContext {
            file_contents: redacted_contents,
            project_docs: baseline.project_docs.clone(),
            commit_log: baseline.commit_log.clone(),
            pr_summary: baseline.pr_summary.clone(),
        },
        repo_root: repo_root.to_string(),
        is_path_scan,
    };

    Ok((ctx, secret_findings))
}

/// Phase 1 (pattern) + Phase 2 (LLM triage) threat scan.
async fn run_threat_scan(
    config: &Config,
    options: &ReviewOptions,
    review_context: &models::context::ReviewContext<'_>,
    provider: &dyn ReviewProvider,
) -> (Vec<models::finding::Finding>, models::TokenUsage) {
    let mut threat_rules = threat::rules::default_rules();
    let config_threat_path: Option<PathBuf> =
        config.threats.additional_rules.as_ref().map(PathBuf::from);
    let threat_rules_path = options
        .threat_rules
        .as_deref()
        .or(config_threat_path.as_deref());
    if let Some(path) = threat_rules_path {
        match threat::rules::load_rules_from_file(path) {
            Ok(extra) => threat_rules.extend(extra),
            Err(e) => eprintln!("Warning: failed to load threat rules: {e}"),
        }
    }

    let raw_matches = threat::scanner::scan_for_threats(
        &review_context.diffs,
        &review_context.baseline.file_contents,
        &threat_rules,
    );

    if raw_matches.is_empty() {
        return (Vec::new(), models::TokenUsage::default());
    }

    if options.show_threat_progress {
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

    let (triaged, triage_tokens) = threat::triage::triage_findings(
        raw_matches,
        &review_context.baseline.file_contents,
        provider,
        config.provider.model_for(crate::config::ModelTask::Triage),
    )
    .await;

    if options.show_threat_progress {
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

    (
        triaged
            .iter()
            .map(threat::match_to_finding)
            .collect::<Vec<_>>(),
        triage_tokens,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TokenUsage;
    use crate::providers::{
        ProviderError, ReviewOutcome, ReviewProvider, TriageOutcome, TriageVerdict,
    };

    /// A test double for [`auto_triage_select`]: returns canned triage
    /// verdicts + token usage, or a forced error. `review`/`summarize` are
    /// never called on this path.
    struct StubTriageProvider {
        verdicts: Vec<TriageVerdict>,
        tokens: TokenUsage,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl ReviewProvider for StubTriageProvider {
        async fn review(
            &self,
            _agent: &AgentDefinition,
            _prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> std::result::Result<ReviewOutcome, ProviderError> {
            unimplemented!("review is not exercised by auto_triage_select tests")
        }

        async fn triage(
            &self,
            _model: &str,
            _system_prompt: &str,
            _user_prompt: &str,
        ) -> std::result::Result<TriageOutcome, ProviderError> {
            if self.fail {
                Err(ProviderError::ApiError("boom".into()))
            } else {
                Ok(TriageOutcome {
                    verdicts: self.verdicts.clone(),
                    tokens: self.tokens,
                })
            }
        }
    }

    fn verdict(classification: &str) -> TriageVerdict {
        TriageVerdict {
            index: 0,
            classification: classification.to_string(),
            rationale: None,
        }
    }

    /// A non-`auto` profile set makes no selection LLM call, so it must
    /// report zero selection tokens and no selection model — the signal
    /// `main` uses to decide whether to fold anything into the totals.
    #[tokio::test]
    async fn resolve_agents_explicit_profiles_spend_no_selection_tokens() {
        let options = ReviewOptions {
            profiles: vec!["backend".to_string()],
            ..Default::default()
        };
        let config = Config::default();
        let resolved = resolve_agents(None, &options, &config, &[], Path::new("."))
            .await
            .expect("resolve explicit profile");
        assert!(resolved.agents.iter().any(|a| a.profile.name == "backend"));
        assert_eq!(resolved.selection_tokens.total(), 0);
        assert!(resolved.selection_model.is_none());
    }

    /// The injected provider's verdicts drive the picked profiles, and its
    /// token usage is propagated for accounting.
    #[tokio::test]
    async fn auto_triage_select_maps_verdicts_and_captures_tokens() {
        let provider = StubTriageProvider {
            verdicts: vec![verdict("backend")],
            tokens: TokenUsage {
                input: 100,
                output: 20,
                ..Default::default()
            },
            fail: false,
        };
        let (profiles, tokens) = auto_triage_select(
            &provider,
            "triage-model",
            "sys",
            "summary",
            vec!["general".to_string()],
        )
        .await;
        assert!(profiles.contains(&"backend".to_string()));
        assert_eq!(tokens.total(), 120);
    }

    /// Tokens are reported even when the verdicts are unusable and we fall
    /// back to the heuristic — the call still cost money. This is the path
    /// that was previously untestable.
    #[tokio::test]
    async fn auto_triage_select_reports_tokens_even_when_falling_back() {
        let provider = StubTriageProvider {
            verdicts: vec![verdict("not-a-real-profile")],
            tokens: TokenUsage {
                input: 50,
                output: 10,
                ..Default::default()
            },
            fail: false,
        };
        let (profiles, tokens) = auto_triage_select(
            &provider,
            "triage-model",
            "sys",
            "summary",
            vec!["general".to_string()],
        )
        .await;
        assert_eq!(profiles, vec!["general".to_string()]); // heuristic fallback
        assert_eq!(tokens.total(), 60); // but tokens still counted
    }

    /// A provider error fails open to the heuristic with zero tokens.
    #[tokio::test]
    async fn auto_triage_select_fails_open_with_zero_tokens() {
        let provider = StubTriageProvider {
            verdicts: vec![],
            tokens: TokenUsage::default(),
            fail: true,
        };
        let (profiles, tokens) = auto_triage_select(
            &provider,
            "triage-model",
            "sys",
            "summary",
            vec!["backend".to_string()],
        )
        .await;
        assert_eq!(profiles, vec!["backend".to_string()]);
        assert_eq!(tokens.total(), 0);
    }
}
