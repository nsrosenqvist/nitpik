//! Review orchestrator: parallel execution and deduplication.
//!
//! # Bounded Context: Review Orchestration
//!
//! Owns task scheduling (parallel `JoinSet` + semaphore), prompt
//! assembly, retry logic, deduplication, and result aggregation.
//! Coordinates `providers`, `agents`, `cache`, and `progress` but
//! delegates all domain work to them.
//!
//! Prompt construction lives in [`prompt`], diff-scope filtering in [`scope`].

pub mod dedup;
pub mod prompt;
pub mod scope;
pub mod verify;

use std::collections::BTreeMap;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::cache::{self, CacheEngine};
use crate::config::Config;
use crate::diff::chunker;
use crate::models::AgentDefinition;
use crate::models::TokenUsage;
use crate::models::context::ReviewContext;
use crate::models::finding::Finding;
use crate::progress::{ProgressReporter, TaskStatus};
use crate::providers::ReviewProvider;
use crate::providers::events::{self, LoopEvent, LoopEventSink};
use crate::providers::response::{classify_error, is_retryable, retry_backoff};

use crate::constants::MAX_RETRIES;

use prompt::{build_prompt, build_prompt_with_prior, build_system_addendum};
use scope::filter_to_diff_scope;
use verify::DroppedFinding;

/// Errors from the orchestrator.
#[derive(Error, Debug)]
pub enum OrchestratorError {
    #[error("provider error: {0}")]
    Provider(#[from] crate::providers::ProviderError),

    #[error("no diffs to review")]
    NoDiffs,
}

/// Result of a review run, including partial results from failed tasks.
#[derive(Debug)]
pub struct ReviewResult {
    /// Deduplicated findings from successful reviews.
    pub findings: Vec<Finding>,
    /// Number of file×agent tasks that failed after retries.
    pub failed_tasks: usize,
    /// Aggregated token usage across every file×agent task that
    /// reached the provider (cache hits contribute zero).
    pub tokens: TokenUsage,
    /// Per-model breakdown of token usage. Profiles can override the
    /// default model via their YAML frontmatter, so a single review
    /// run may consume tokens on several models. The map is keyed by
    /// the resolved model identifier.
    pub tokens_by_model: BTreeMap<String, TokenUsage>,
    /// Findings the critic dropped (only populated when `verify`
    /// was enabled on the orchestrator). Empty otherwise.
    pub dropped: Vec<DroppedFinding>,
}

/// Orchestrates parallel review execution across agents and files.
pub struct ReviewOrchestrator {
    provider: Arc<dyn ReviewProvider>,
    config: Config,
    cache: Arc<CacheEngine>,
    progress: Arc<dyn ProgressReporter>,
    /// When `true`, skip injecting prior findings into the prompt.
    no_prior_context: bool,
    /// Optional cap on how many prior findings are included.
    max_prior_findings: Option<usize>,
    /// Branch / PR scope for sidecar isolation.
    review_scope: String,
    /// When `true`, run a critic pass over deduped findings to drop
    /// probable false positives.
    verify: bool,
    /// When `true`, profiles with `wave: 2` run after wave 1 and
    /// receive wave-1 findings as additional context. Off by default.
    multi_wave: bool,
}

impl ReviewOrchestrator {
    /// Create a new orchestrator.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn ReviewProvider>,
        config: &Config,
        cache: CacheEngine,
        progress: Arc<dyn ProgressReporter>,
        no_prior_context: bool,
        max_prior_findings: Option<usize>,
        review_scope: String,
        verify: bool,
        multi_wave: bool,
    ) -> Self {
        Self {
            provider,
            config: config.clone(),
            cache: Arc::new(cache),
            progress,
            no_prior_context,
            max_prior_findings,
            review_scope,
            verify,
            multi_wave,
        }
    }

    /// Run all agents across all files and return deduplicated findings.
    ///
    /// Returns a `ReviewResult` containing findings from successful reviews
    /// and a count of tasks that failed. Callers should check `failed_tasks`
    /// to decide whether to fail the pipeline.
    pub async fn run(
        &self,
        context: &ReviewContext<'_>,
        agents: &[AgentDefinition],
        max_concurrent: usize,
        agentic: bool,
        max_turns: usize,
        max_tool_calls: usize,
    ) -> Result<ReviewResult, OrchestratorError> {
        if context.diffs.is_empty() {
            return Err(OrchestratorError::NoDiffs);
        }

        // Clear the cross-task tool-result memo so stale entries from
        // prior runs (in long-lived processes such as `nitpik watch`
        // or test harnesses) can't leak into this review.
        crate::tools::memo::clear();

        // Static system addendum (project docs + commit log) is
        // identical across every task in this run. Build once and
        // splice into each task's agent system prompt so providers
        // can cache the system prefix.
        let system_addendum = build_system_addendum(context);

        // Partition agents into waves when --multi-wave is enabled.
        // Otherwise every agent runs in wave 1.
        let (wave1_agents, wave2_agents): (Vec<&AgentDefinition>, Vec<&AgentDefinition>) =
            if self.multi_wave {
                agents.iter().partition(|a| a.profile.wave <= 1)
            } else {
                (agents.iter().collect(), Vec::new())
            };

        let mut all_findings: Vec<Finding> = Vec::new();
        let mut failed_count: usize = 0;
        let mut total_tokens = TokenUsage::default();
        let mut tokens_by_model: BTreeMap<String, TokenUsage> = BTreeMap::new();

        let (w1_findings, w1_failed, w1_tokens, w1_tokens_by_model) = self
            .dispatch_wave(
                context,
                &wave1_agents,
                agents,
                &system_addendum,
                None,
                max_concurrent,
                agentic,
                max_turns,
                max_tool_calls,
            )
            .await;
        failed_count += w1_failed;
        total_tokens += w1_tokens;
        for (m, t) in w1_tokens_by_model {
            *tokens_by_model.entry(m).or_default() += t;
        }

        if !wave2_agents.is_empty() && !w1_findings.is_empty() {
            // Build a compact summary of wave-1 findings to feed wave-2
            // reviewers as extra context.
            let wave1_summary = format_wave1_summary(&w1_findings);
            let (w2_findings, w2_failed, w2_tokens, w2_tokens_by_model) = self
                .dispatch_wave(
                    context,
                    &wave2_agents,
                    agents,
                    &system_addendum,
                    Some(&wave1_summary),
                    max_concurrent,
                    agentic,
                    max_turns,
                    max_tool_calls,
                )
                .await;
            failed_count += w2_failed;
            total_tokens += w2_tokens;
            for (m, t) in w2_tokens_by_model {
                *tokens_by_model.entry(m).or_default() += t;
            }
            all_findings.extend(w2_findings);
        } else if !wave2_agents.is_empty() {
            // Wave 1 found nothing; run wave 2 anyway with no addendum.
            let (w2_findings, w2_failed, w2_tokens, w2_tokens_by_model) = self
                .dispatch_wave(
                    context,
                    &wave2_agents,
                    agents,
                    &system_addendum,
                    None,
                    max_concurrent,
                    agentic,
                    max_turns,
                    max_tool_calls,
                )
                .await;
            failed_count += w2_failed;
            total_tokens += w2_tokens;
            for (m, t) in w2_tokens_by_model {
                *tokens_by_model.entry(m).or_default() += t;
            }
            all_findings.extend(w2_findings);
        }

        all_findings.extend(w1_findings);

        // Deduplicate findings
        let deduped = dedup::deduplicate(all_findings);

        // Filter out findings outside diff boundaries (skip for path-based scans
        // where all file content is in scope)
        let scoped = if context.is_path_scan {
            deduped
        } else {
            filter_to_diff_scope(deduped, &context.diffs)
        };

        // Optional critic pass: drop findings the critic votes to
        // discard. Fails open on provider error.
        let (final_findings, dropped) = if self.verify && !scoped.is_empty() {
            let outcome = verify::verify_findings(&self.provider, scoped).await;
            if outcome.tokens.total() > 0 {
                total_tokens += outcome.tokens;
                let critic_model = self.config.provider.resolved_model().to_string();
                *tokens_by_model.entry(critic_model).or_default() += outcome.tokens;
            }
            (outcome.kept, outcome.dropped)
        } else {
            (scoped, Vec::new())
        };

        Ok(ReviewResult {
            findings: final_findings,
            failed_tasks: failed_count,
            tokens: total_tokens,
            tokens_by_model,
            dropped,
        })
    }

    /// Dispatch a single wave of file×agent tasks and collect results.
    ///
    /// `wave_addendum` is appended to each agent's system prompt
    /// before tasks are spawned. Returns aggregated findings, failure
    /// count, total tokens, and per-model token breakdown.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_wave(
        &self,
        context: &ReviewContext<'_>,
        wave_agents: &[&AgentDefinition],
        all_agents: &[AgentDefinition],
        system_addendum: &str,
        wave_addendum: Option<&str>,
        max_concurrent: usize,
        agentic: bool,
        max_turns: usize,
        max_tool_calls: usize,
    ) -> (
        Vec<Finding>,
        usize,
        TokenUsage,
        BTreeMap<String, TokenUsage>,
    ) {
        if wave_agents.is_empty() {
            return (Vec::new(), 0, TokenUsage::default(), BTreeMap::new());
        }

        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let mut join_set = JoinSet::new();

        struct Task<'a> {
            chunk: crate::models::diff::FileDiff<'a>,
            agent: AgentDefinition,
            line_count: usize,
        }
        let mut tasks: Vec<Task<'_>> = Vec::new();
        for agent in wave_agents {
            for diff in &context.diffs {
                if diff.is_binary {
                    continue;
                }
                let chunks = chunker::chunk_diff(diff, None);
                for chunk in chunks {
                    let line_count: usize = chunk.hunks.iter().map(|h| h.lines.len()).sum();
                    tasks.push(Task {
                        chunk,
                        agent: (*agent).clone(),
                        line_count,
                    });
                }
            }
        }
        tasks.sort_by_key(|t| t.line_count);

        for Task {
            chunk, mut agent, ..
        } in tasks
        {
            let provider = Arc::clone(&self.provider);
            let sem = Arc::clone(&semaphore);
            let cache = Arc::clone(&self.cache);
            let progress = Arc::clone(&self.progress);
            let no_prior_context = self.no_prior_context;
            let max_prior_findings = self.max_prior_findings;
            let review_scope = self.review_scope.clone();
            let model = agent
                .profile
                .model
                .as_deref()
                .unwrap_or_else(|| self.config.provider.resolved_model())
                .to_string();
            let file_path = chunk.path().to_string();

            // Augment the per-task agent's system prompt with the
            // static (run-wide) context. The result remains constant
            // across every file for the same agent, satisfying the
            // cacheability requirement.
            if !system_addendum.is_empty() {
                if !agent.system_prompt.ends_with("\n\n") {
                    if agent.system_prompt.ends_with('\n') {
                        agent.system_prompt.push('\n');
                    } else {
                        agent.system_prompt.push_str("\n\n");
                    }
                }
                agent.system_prompt.push_str(system_addendum);
            }
            // Wave-2 agents also receive the wave-1 findings summary.
            if let Some(addendum) = wave_addendum {
                if !agent.system_prompt.ends_with("\n\n") {
                    if agent.system_prompt.ends_with('\n') {
                        agent.system_prompt.push('\n');
                    } else {
                        agent.system_prompt.push_str("\n\n");
                    }
                }
                agent.system_prompt.push_str(addendum);
            }

            let base_prompt = build_prompt(&chunk, context, &agent, all_agents, None, agentic);
            let cache_key = cache::cache_key(&base_prompt, &agent.profile.name, &model);

            join_set.spawn(execute_review_task(ReviewTaskParams {
                provider,
                cache,
                progress,
                sem,
                file_path,
                agent,
                model,
                cache_key,
                review_scope,
                base_prompt,
                no_prior_context,
                max_prior_findings,
                agentic,
                max_turns,
                max_tool_calls,
            }));
        }

        let mut findings: Vec<Finding> = Vec::new();
        let mut failed: usize = 0;
        let mut total_tokens = TokenUsage::default();
        let mut tokens_by_model: BTreeMap<String, TokenUsage> = BTreeMap::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok((f, fail, tokens, model)) => {
                    findings.extend(f);
                    total_tokens += tokens;
                    if tokens.total() > 0 {
                        *tokens_by_model.entry(model).or_default() += tokens;
                    }
                    if fail {
                        failed += 1;
                    }
                }
                Err(e) => {
                    eprintln!("Warning: review task panicked: {e}");
                    failed += 1;
                }
            }
        }
        (findings, failed, total_tokens, tokens_by_model)
    }
}

/// Build a compact summary of wave-1 findings to feed wave-2 reviewers.
///
/// Caps both the number of findings included and each finding's
/// message length so the addendum stays well within the system-prompt
/// budget even when wave 1 produced hundreds of findings.
fn format_wave1_summary(findings: &[Finding]) -> String {
    const MAX_FINDINGS: usize = 30;
    const MAX_MSG: usize = 240;
    let mut s = String::with_capacity(findings.len().min(MAX_FINDINGS) * 200);
    s.push_str(
        "## Findings from initial review\n\n\
         The following findings were produced by an earlier wave of \
         reviewers. Use them as context: cross-reference, build on, \
         or contradict them. Do not duplicate findings already covered \
         here unless you have new evidence.\n\n",
    );
    for f in findings.iter().take(MAX_FINDINGS) {
        let mut msg: String = f.message.chars().take(MAX_MSG).collect();
        if f.message.chars().count() > MAX_MSG {
            msg.push('…');
        }
        s.push_str(&format!(
            "- **{}** [{}] `{}:{}` — {}: {}\n",
            f.agent, f.severity, f.file, f.line, f.title, msg
        ));
    }
    if findings.len() > MAX_FINDINGS {
        s.push_str(&format!(
            "- … and {} more findings omitted for brevity\n",
            findings.len() - MAX_FINDINGS
        ));
    }
    s
}

/// Parameters for a single file×agent review task.
struct ReviewTaskParams {
    provider: Arc<dyn ReviewProvider>,
    cache: Arc<CacheEngine>,
    progress: Arc<dyn ProgressReporter>,
    sem: Arc<Semaphore>,
    file_path: String,
    agent: AgentDefinition,
    model: String,
    cache_key: String,
    review_scope: String,
    base_prompt: String,
    no_prior_context: bool,
    max_prior_findings: Option<usize>,
    agentic: bool,
    max_turns: usize,
    max_tool_calls: usize,
}

/// Execute a single file×agent review task with caching and retries.
async fn execute_review_task(params: ReviewTaskParams) -> (Vec<Finding>, bool, TokenUsage, String) {
    let ReviewTaskParams {
        provider,
        cache,
        progress,
        sem,
        file_path,
        agent,
        model,
        cache_key,
        review_scope,
        base_prompt,
        no_prior_context,
        max_prior_findings,
        agentic,
        max_turns,
        max_tool_calls,
    } = params;
    // Check cache first
    if let Some(cached) = cache.get(&cache_key).await {
        cache
            .put_sidecar(
                &file_path,
                &agent.profile.name,
                &model,
                &cache_key,
                &review_scope,
            )
            .await;
        progress.update(&file_path, TaskStatus::Done);
        return (cached, false, TokenUsage::default(), model);
    }

    // Cache miss — resolve prior findings for the prompt
    let prompt = if no_prior_context {
        base_prompt.clone()
    } else {
        let prior = cache
            .get_previous(
                &file_path,
                &agent.profile.name,
                &model,
                &cache_key,
                &review_scope,
            )
            .await;
        match prior {
            Some(mut findings) if !findings.is_empty() => {
                findings.sort_by(|a, b| b.severity.cmp(&a.severity));
                if let Some(cap) = max_prior_findings {
                    findings.truncate(cap);
                }
                build_prompt_with_prior(&base_prompt, &findings)
            }
            _ => base_prompt.clone(),
        }
    };

    progress.update(&file_path, TaskStatus::InProgress);
    let _permit = sem.acquire().await.expect("semaphore closed");

    match with_retry(
        &provider,
        &agent,
        &prompt,
        agentic,
        max_turns,
        max_tool_calls,
        &progress,
        &file_path,
    )
    .await
    {
        Ok((findings, tokens)) => {
            cache.put(&cache_key, &findings).await;
            cache
                .put_sidecar(
                    &file_path,
                    &agent.profile.name,
                    &model,
                    &cache_key,
                    &review_scope,
                )
                .await;
            progress.update(&file_path, TaskStatus::Done);
            (findings, false, tokens, model)
        }
        Err(err_msg) => {
            progress.update(&file_path, TaskStatus::Failed(err_msg));
            (Vec::new(), true, TokenUsage::default(), model)
        }
    }
}

/// Retry a provider review call with exponential backoff.
///
/// Returns `Ok((findings, tokens))` on success or `Err(message)` when retries
/// are exhausted or a non-retryable error is encountered.
#[allow(clippy::too_many_arguments)] // Thin extraction from spawn closure; a one-shot struct adds noise.
async fn with_retry(
    provider: &Arc<dyn ReviewProvider>,
    agent: &AgentDefinition,
    prompt: &str,
    agentic: bool,
    max_turns: usize,
    max_tool_calls: usize,
    progress: &Arc<dyn ProgressReporter>,
    file_path: &str,
) -> Result<(Vec<Finding>, TokenUsage), String> {
    let mut last_err = None;

    let sink: LoopEventSink = {
        let progress = Arc::clone(progress);
        let file_path = file_path.to_string();
        Arc::new(move |ev| match ev {
            LoopEvent::ToolCallStart { tool, .. } => {
                progress.update(&file_path, TaskStatus::ToolCalling { tool });
            }
            LoopEvent::ToolCallEnd { .. } => {
                progress.update(&file_path, TaskStatus::InProgress);
            }
            _ => {}
        })
    };

    for attempt in 0..=MAX_RETRIES {
        let call = provider.review(agent, prompt, agentic, max_turns, max_tool_calls);
        match events::scope(Some(Arc::clone(&sink)), call).await {
            Ok(outcome) => return Ok((outcome.findings, outcome.tokens)),
            Err(ref e) if is_retryable(e) && attempt < MAX_RETRIES => {
                let backoff = retry_backoff(attempt);
                let reason = classify_error(e).unwrap_or("Transient error").to_string();
                progress.update(
                    file_path,
                    TaskStatus::Retrying {
                        attempt: attempt + 1,
                        max: MAX_RETRIES + 1,
                        reason,
                        backoff_secs: backoff.as_secs(),
                    },
                );
                tokio::time::sleep(backoff).await;
                progress.update(file_path, TaskStatus::InProgress);
                last_err = Some(format!("{e}"));
            }
            Err(e) => {
                let short = classify_error(&e)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{e}"));
                return Err(short);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| "max retries exhausted".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::finding::Severity;

    fn f(agent: &str, file: &str, line: u32, title: &str, msg: &str) -> Finding {
        Finding {
            file: file.into(),
            line,
            end_line: None,
            severity: Severity::Warning,
            title: title.into(),
            message: msg.into(),
            suggestion: None,
            agent: agent.into(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn wave1_summary_includes_each_finding_with_agent_and_severity() {
        let findings = vec![
            f("backend", "src/a.rs", 10, "issue a", "details a"),
            f("frontend", "src/b.tsx", 20, "issue b", "details b"),
        ];
        let s = format_wave1_summary(&findings);
        assert!(s.contains("## Findings from initial review"));
        assert!(s.contains("**backend**"));
        assert!(s.contains("**frontend**"));
        assert!(s.contains("src/a.rs:10"));
        assert!(s.contains("src/b.tsx:20"));
        assert!(s.contains("issue a"));
        assert!(s.contains("issue b"));
    }

    #[test]
    fn wave1_summary_caps_findings_count() {
        let findings: Vec<Finding> = (0..50)
            .map(|i| f("backend", "x.rs", i + 1, &format!("t{i}"), "m"))
            .collect();
        let s = format_wave1_summary(&findings);
        // Should mention 30 included + omitted suffix.
        assert!(s.contains("and 20 more findings omitted"));
        assert!(s.contains("t0"));
        assert!(!s.contains("t30"), "findings beyond cap should be omitted");
    }

    #[test]
    fn wave1_summary_truncates_long_messages() {
        let long: String = "x".repeat(500);
        let findings = vec![f("backend", "f.rs", 1, "t", &long)];
        let s = format_wave1_summary(&findings);
        // The truncation marker must be present.
        assert!(s.contains('…'));
        // And the full 500x string must NOT appear.
        assert!(!s.contains(&"x".repeat(500)));
    }
}
