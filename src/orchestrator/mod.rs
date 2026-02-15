//! Review orchestrator: parallel execution, prompt construction, and deduplication.

pub mod dedup;

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::cache::{self, CacheEngine};
use crate::config::Config;
use crate::diff::chunker;
use crate::models::context::ReviewContext;
use crate::models::diff::FileDiff;
use crate::models::finding::Finding;
use crate::models::AgentDefinition;
use crate::progress::{ProgressTracker, TaskStatus};
use crate::providers::rig::{classify_error, is_retryable, retry_backoff, MAX_RETRIES};
use crate::providers::ReviewProvider;

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
    progress: Arc<ProgressTracker>,
    /// When `true`, skip injecting prior findings into the prompt.
    no_prior_context: bool,
    /// Optional cap on how many prior findings are included.
    max_prior_findings: Option<usize>,
}

impl ReviewOrchestrator {
    /// Create a new orchestrator.
    pub fn new(
        provider: Arc<dyn ReviewProvider>,
        config: &Config,
        cache: CacheEngine,
        progress: Arc<ProgressTracker>,
        no_prior_context: bool,
        max_prior_findings: Option<usize>,
    ) -> Self {
        Self {
            provider,
            config: config.clone(),
            cache: Arc::new(cache),
            progress,
            no_prior_context,
            max_prior_findings,
        }
    }

    /// Run all agents across all files and return deduplicated findings.
    ///
    /// Returns a `ReviewResult` containing findings from successful reviews
    /// and a count of tasks that failed. Callers should check `failed_tasks`
    /// to decide whether to fail the pipeline.
    pub async fn run(
        &self,
        context: &ReviewContext,
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

        // For each agent × file combination, spawn a concurrent task
        for agent in agents {
            for diff in &context.diffs {
                if diff.is_binary {
                    continue;
                }

                // Split large diffs into chunks
                let chunks = chunker::chunk_diff(diff, None);

                for chunk in chunks {
                    let provider = Arc::clone(&self.provider);
                    let sem = Arc::clone(&semaphore);
                    let cache = Arc::clone(&self.cache);
                    let progress = Arc::clone(&self.progress);
                    let agent = agent.clone();
                    let no_prior_context = self.no_prior_context;
                    let max_prior_findings = self.max_prior_findings;
                    let model = agent
                        .profile
                        .model
                        .as_deref()
                        .unwrap_or(&self.config.provider.model)
                        .to_string();
                    let file_path = chunk.path().to_string();

                    // Build the base prompt (without prior findings —
                    // they are injected below only on cache miss).
                    let base_prompt = build_prompt(&chunk, context, &agent, None);

                    // Compute content for cache key from the base prompt (deterministic)
                    let cache_key = cache::cache_key(&base_prompt, &agent.profile.name, &model);

                    join_set.spawn(async move {
                        // Check cache first
                        if let Some(cached) = cache.get(&cache_key) {
                            // Sidecar stays current — write it in case
                            // this is the first run with sidecar support.
                            cache.put_sidecar(&file_path, &agent.profile.name, &model, &cache_key);
                            progress.update(&file_path, TaskStatus::Done);
                            return (cached, false);
                        }

                        // Cache miss — look up prior findings from the
                        // previous (now-invalidated) cache entry.
                        let prompt = if no_prior_context {
                            base_prompt
                        } else {
                            let prior = cache.get_previous(
                                &file_path,
                                &agent.profile.name,
                                &model,
                                &cache_key,
                            );
                            match prior {
                                Some(mut findings) if !findings.is_empty() => {
                                    // Sort by severity (errors first) before capping
                                    findings.sort_by(|a, b| b.severity.cmp(&a.severity));
                                    if let Some(cap) = max_prior_findings {
                                        findings.truncate(cap);
                                    }
                                    build_prompt_with_prior(&base_prompt, &findings)
                                }
                                _ => base_prompt,
                            }
                        };

                        progress.update(&file_path, TaskStatus::InProgress);
                        let _permit = sem.acquire().await.expect("semaphore closed");

                        let mut last_err = None;

                        for attempt in 0..=MAX_RETRIES {
                            match provider.review(&agent, &prompt, agentic, max_turns, max_tool_calls).await {
                                Ok(findings) => {
                                    cache.put(&cache_key, &findings);
                                    cache.put_sidecar(&file_path, &agent.profile.name, &model, &cache_key);
                                    progress.update(&file_path, TaskStatus::Done);
                                    return (findings, false);
                                }
                                Err(ref e) if is_retryable(e) && attempt < MAX_RETRIES => {
                                    let backoff = retry_backoff(attempt);
                                    let reason = classify_error(e)
                                        .unwrap_or("Transient error")
                                        .to_string();
                                    progress.update(
                                        &file_path,
                                        TaskStatus::Retrying {
                                            attempt: attempt + 1,
                                            max: MAX_RETRIES + 1,
                                            reason,
                                            backoff_secs: backoff.as_secs(),
                                        },
                                    );
                                    tokio::time::sleep(backoff).await;
                                    // Restore in-progress after backoff
                                    progress.update(&file_path, TaskStatus::InProgress);
                                    last_err = Some(format!("{e}"));
                                }
                                Err(e) => {
                                    let short = classify_error(&e)
                                        .map(|s| s.to_string())
                                        .unwrap_or_else(|| format!("{e}"));
                                    progress.update(
                                        &file_path,
                                        TaskStatus::Failed(short),
                                    );
                                    return (Vec::new(), true);
                                }
                            }
                        }

                        // Exhausted all retries
                        progress.update(
                            &file_path,
                            TaskStatus::Failed(
                                last_err.unwrap_or_else(|| "max retries exhausted".to_string()),
                            ),
                        );
                        (Vec::new(), true)
                    });
                }
            }
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

        Ok(ReviewResult {
            findings: deduped,
            failed_tasks: failed_count,
        })
    }
}

/// Build the user prompt for a single file review.
fn build_prompt(
    diff: &FileDiff,
    context: &ReviewContext,
    agent: &AgentDefinition,
    previous_findings: Option<&[Finding]>,
) -> String {
    let mut prompt = String::new();

    // Project docs context
    if !context.baseline.project_docs.is_empty() {
        prompt.push_str("## Project Documentation\n\n");
        for (name, content) in &context.baseline.project_docs {
            prompt.push_str(&format!("### {name}\n\n{content}\n\n"));
        }
    }

    // Full file content (if available)
    let file_path = diff.path();
    if let Some(content) = context.baseline.file_contents.get(file_path) {
        prompt.push_str(&format!(
            "## Full File Content: {file_path}\n\n```\n{content}\n```\n\n"
        ));
    }

    // The diff itself
    prompt.push_str(&format!("## Diff for: {file_path}\n\n```diff\n"));
    for hunk in &diff.hunks {
        if let Some(ref header) = hunk.header {
            prompt.push_str(&format!(
                "@@ -{},{} +{},{} @@ {header}\n",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            ));
        } else {
            prompt.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
            ));
        }
        for line in &hunk.lines {
            let prefix = match line.line_type {
                crate::models::diff::DiffLineType::Added => "+",
                crate::models::diff::DiffLineType::Removed => "-",
                crate::models::diff::DiffLineType::Context => " ",
            };
            prompt.push_str(&format!("{prefix}{}\n", line.content));
        }
    }
    prompt.push_str("```\n\n");

    // Previous findings (if any)
    if let Some(findings) = previous_findings {
        if !findings.is_empty() {
            prompt.push_str(&format_prior_findings_section(findings));
        }
    }

    // Instructions
    prompt.push_str(&format!(
        "## Instructions\n\n\
        Review the diff above for file `{file_path}`. \
        You are the **{}** reviewer: {}\n\n\
        Return your findings as a JSON array. For each finding include:\n\
        - \"file\": the file path (\"{}\")\n\
        - \"line\": the line number in the new file\n\
        - \"severity\": MUST be exactly one of: \"error\", \"warning\", \"info\"\n\
        - \"title\": short summary of the issue\n\
        - \"message\": detailed explanation\n\
        - \"suggestion\": (optional) suggested fix\n\
        - \"agent\": \"{}\"\n\n\
        IMPORTANT: The \"severity\" field must be one of \"error\", \"warning\", or \"info\". \
        Do NOT use values like \"critical\", \"major\", \"minor\", \"high\", or \"low\".\n\n\
        If there are no issues, return an empty array: []\n",
        agent.profile.name,
        agent.profile.description,
        file_path,
        agent.profile.name,
    ));

    prompt
}

/// Append the prior-findings section to an already-built base prompt.
///
/// This is used on cache miss when prior findings are available,
/// so the cache key (computed from the base prompt) stays stable.
fn build_prompt_with_prior(base_prompt: &str, findings: &[Finding]) -> String {
    let mut prompt = base_prompt.to_string();
    // Insert the prior findings section just before the "## Instructions" header
    if let Some(pos) = prompt.find("## Instructions") {
        prompt.insert_str(pos, &format_prior_findings_section(findings));
    } else {
        // Fallback: append at the end
        prompt.push_str(&format_prior_findings_section(findings));
    }
    prompt
}

/// Format the "Previous Review Findings" prompt section.
fn format_prior_findings_section(findings: &[Finding]) -> String {
    let json = serde_json::to_string_pretty(findings).unwrap_or_else(|_| "[]".to_string());
    format!(
        "## Previous Review Findings\n\n\
        The following findings were reported in a previous review of this file. \
        The file has changed since then.\n\n\
        - **Re-raise** any findings that still apply to the current diff.\n\
        - **Drop** any findings that have been resolved by the changes.\n\
        - **Add** any genuinely new issues introduced by the current changes.\n\
        - Do **not** duplicate previous findings that are unchanged.\n\n\
        ```json\n{json}\n```\n\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::context::BaselineContext;
    use crate::models::diff::{DiffLine, DiffLineType, Hunk};

    #[test]
    fn build_prompt_includes_diff() {
        let diff = FileDiff {
            old_path: "test.rs".into(),
            new_path: "test.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: "let x = 1;".into(),
                    old_line_no: None,
                    new_line_no: Some(1),
                }],
            }],
        };
        let context = ReviewContext {
            diffs: vec![diff.clone()],
            baseline: BaselineContext::default(),
            repo_root: "/tmp".into(),
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, None);
        assert!(prompt.contains("+let x = 1;"));
        assert!(prompt.contains("test.rs"));
        assert!(prompt.contains("backend"));
    }

    #[test]
    fn build_prompt_includes_prior_findings() {
        let diff = FileDiff {
            old_path: "test.rs".into(),
            new_path: "test.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: "let x = 1;".into(),
                    old_line_no: None,
                    new_line_no: Some(1),
                }],
            }],
        };
        let context = ReviewContext {
            diffs: vec![diff.clone()],
            baseline: BaselineContext::default(),
            repo_root: "/tmp".into(),
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();
        let prior = vec![Finding {
            file: "test.rs".into(),
            line: 1,
            end_line: None,
            severity: crate::models::finding::Severity::Warning,
            title: "Old issue".into(),
            message: "This was found before".into(),
            suggestion: None,
            agent: "backend".into(),
        }];

        let prompt = build_prompt(&diff, &context, &agent, Some(&prior));
        assert!(prompt.contains("Previous Review Findings"));
        assert!(prompt.contains("Old issue"));
        assert!(prompt.contains("Re-raise"));
    }

    #[test]
    fn build_prompt_excludes_prior_when_none() {
        let diff = FileDiff {
            old_path: "test.rs".into(),
            new_path: "test.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: "let x = 1;".into(),
                    old_line_no: None,
                    new_line_no: Some(1),
                }],
            }],
        };
        let context = ReviewContext {
            diffs: vec![diff.clone()],
            baseline: BaselineContext::default(),
            repo_root: "/tmp".into(),
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, None);
        assert!(!prompt.contains("Previous Review Findings"));
    }

    #[test]
    fn build_prompt_with_prior_injects_before_instructions() {
        let diff = FileDiff {
            old_path: "test.rs".into(),
            new_path: "test.rs".into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                header: None,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Added,
                    content: "let x = 1;".into(),
                    old_line_no: None,
                    new_line_no: Some(1),
                }],
            }],
        };
        let context = ReviewContext {
            diffs: vec![diff.clone()],
            baseline: BaselineContext::default(),
            repo_root: "/tmp".into(),
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();
        let prior = vec![Finding {
            file: "test.rs".into(),
            line: 5,
            end_line: None,
            severity: crate::models::finding::Severity::Error,
            title: "Critical bug".into(),
            message: "Needs fixing".into(),
            suggestion: None,
            agent: "backend".into(),
        }];

        let base = build_prompt(&diff, &context, &agent, None);
        let with_prior = build_prompt_with_prior(&base, &prior);

        // Prior findings section should appear before Instructions
        let prior_pos = with_prior.find("Previous Review Findings").unwrap();
        let instr_pos = with_prior.find("## Instructions").unwrap();
        assert!(prior_pos < instr_pos);
        assert!(with_prior.contains("Critical bug"));
    }
}
