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

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::cache::{self, CacheEngine};
use crate::config::Config;
use crate::diff::chunker;
use crate::models::AgentDefinition;
use crate::models::context::ReviewContext;
use crate::models::finding::Finding;
use crate::progress::{ProgressReporter, TaskStatus};
use crate::providers::ReviewProvider;
use crate::providers::response::{classify_error, is_retryable, retry_backoff};

use crate::constants::MAX_RETRIES;

use prompt::{build_prompt, build_prompt_with_prior};
use scope::filter_to_diff_scope;

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
}

impl ReviewOrchestrator {
    /// Create a new orchestrator.
    pub fn new(
        provider: Arc<dyn ReviewProvider>,
        config: &Config,
        cache: CacheEngine,
        progress: Arc<dyn ProgressReporter>,
        no_prior_context: bool,
        max_prior_findings: Option<usize>,
        review_scope: String,
    ) -> Self {
        Self {
            provider,
            config: config.clone(),
            cache: Arc::new(cache),
            progress,
            no_prior_context,
            max_prior_findings,
            review_scope,
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

        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let mut join_set = JoinSet::new();

        // Collect all (chunk, agent) tasks, then sort smallest-first so
        // short tasks fill gaps while large tasks are still running.
        struct Task<'a> {
            chunk: crate::models::diff::FileDiff<'a>,
            agent: AgentDefinition,
            line_count: usize,
        }
        let mut tasks: Vec<Task<'_>> = Vec::new();
        for agent in agents {
            for diff in &context.diffs {
                if diff.is_binary {
                    continue;
                }
                let chunks = chunker::chunk_diff(diff, None);
                for chunk in chunks {
                    let line_count: usize = chunk.hunks.iter().map(|h| h.lines.len()).sum();
                    tasks.push(Task {
                        chunk,
                        agent: agent.clone(),
                        line_count,
                    });
                }
            }
        }
        tasks.sort_by_key(|t| t.line_count);

        for Task { chunk, agent, .. } in tasks {
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

            let base_prompt = build_prompt(&chunk, context, &agent, agents, None, agentic);
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

        // Collect results from all tasks
        let mut all_findings: Vec<Finding> = Vec::new();
        let mut failed_count: usize = 0;
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok((findings, failed)) => {
                    all_findings.extend(findings);
                    if failed {
                        failed_count += 1;
                    }
                }
                Err(e) => {
                    eprintln!("Warning: review task panicked: {e}");
                    failed_count += 1;
                }
            }
        }

        // Deduplicate findings
        let deduped = dedup::deduplicate(all_findings);

        // Filter out findings outside diff boundaries (skip for path-based scans
        // where all file content is in scope)
        let scoped = if context.is_path_scan {
            deduped
        } else {
            filter_to_diff_scope(deduped, &context.diffs)
        };

        Ok(ReviewResult {
            findings: scoped,
            failed_tasks: failed_count,
        })
    }
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
async fn execute_review_task(params: ReviewTaskParams) -> (Vec<Finding>, bool) {
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
        return (cached, false);
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
        Ok(findings) => {
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
            (findings, false)
        }
        Err(err_msg) => {
            progress.update(&file_path, TaskStatus::Failed(err_msg));
            (Vec::new(), true)
        }
    }
}

/// Retry a provider review call with exponential backoff.
///
/// Returns `Ok(findings)` on success or `Err(message)` when retries
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
) -> Result<Vec<Finding>, String> {
    let mut last_err = None;

    for attempt in 0..=MAX_RETRIES {
        match provider
            .review(agent, prompt, agentic, max_turns, max_tool_calls)
            .await
        {
            Ok(findings) => return Ok(findings),
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
