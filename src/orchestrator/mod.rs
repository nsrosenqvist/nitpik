//! Review orchestrator: parallel execution, prompt construction, and deduplication.

pub mod dedup;

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::cache::{self, CacheEngine};
use crate::config::Config;
use crate::diff::chunker;
use crate::models::AgentDefinition;
use crate::models::context::ReviewContext;
use crate::models::diff::FileDiff;
use crate::models::finding::Finding;
use crate::progress::{ProgressTracker, TaskStatus};
use crate::providers::ReviewProvider;
use crate::providers::rig::{MAX_RETRIES, classify_error, is_retryable, retry_backoff};

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
    /// Branch / PR scope for sidecar isolation.
    review_scope: String,
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
                    let review_scope = self.review_scope.clone();
                    let model = agent
                        .profile
                        .model
                        .as_deref()
                        .unwrap_or(&self.config.provider.model)
                        .to_string();
                    let file_path = chunk.path().to_string();

                    // Build the base prompt (without prior findings —
                    // they are injected below only on cache miss).
                    // Agentic context is included in the base prompt so
                    // the LLM always sees tool guidance and changed-file
                    // paths regardless of cache state.
                    let base_prompt = build_prompt(&chunk, context, &agent, agents, None, agentic);

                    // Compute content for cache key from the base prompt (deterministic)
                    let cache_key = cache::cache_key(&base_prompt, &agent.profile.name, &model);

                    join_set.spawn(async move {
                        // Check cache first
                        if let Some(cached) = cache.get(&cache_key) {
                            // Sidecar stays current — write it in case
                            // this is the first run with sidecar support.
                            cache.put_sidecar(
                                &file_path,
                                &agent.profile.name,
                                &model,
                                &cache_key,
                                &review_scope,
                            );
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
                                &review_scope,
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
                            match provider
                                .review(&agent, &prompt, agentic, max_turns, max_tool_calls)
                                .await
                            {
                                Ok(findings) => {
                                    cache.put(&cache_key, &findings);
                                    cache.put_sidecar(
                                        &file_path,
                                        &agent.profile.name,
                                        &model,
                                        &cache_key,
                                        &review_scope,
                                    );
                                    progress.update(&file_path, TaskStatus::Done);
                                    return (findings, false);
                                }
                                Err(ref e) if is_retryable(e) && attempt < MAX_RETRIES => {
                                    let backoff = retry_backoff(attempt);
                                    let reason =
                                        classify_error(e).unwrap_or("Transient error").to_string();
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
                                    progress.update(&file_path, TaskStatus::Failed(short));
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

/// Build the user prompt for a single file review.
fn build_prompt(
    diff: &FileDiff,
    context: &ReviewContext,
    agent: &AgentDefinition,
    all_agents: &[AgentDefinition],
    previous_findings: Option<&[Finding]>,
    agentic: bool,
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

    // Agentic context: help the LLM use tools effectively
    if agentic {
        prompt.push_str(&build_agentic_context(diff, context, agent));
    }

    // Previous findings (if any)
    if let Some(findings) = previous_findings {
        if !findings.is_empty() {
            prompt.push_str(&format_prior_findings_section(findings));
        }
    }

    // Instructions
    let coordination_note = build_coordination_note(agent, all_agents);
    prompt.push_str(&format!(
        "## Instructions\n\n\
        Review the diff above for file `{file_path}`. \
        You are the **{}** reviewer: {}\n\n\
        {coordination_note}\
        IMPORTANT SCOPE RULE: Only report findings on lines that appear in the diff hunks above. \
        The full file content is provided for context only — do NOT flag pre-existing issues in \
        unchanged code outside the diff. Every finding's line number must fall within a diff hunk range.\n\n\
        Prefer precision over recall. If you are uncertain whether something is a real issue, \
        lower the severity to \"info\" or omit it entirely. Do not report hypothetical issues \
        that require runtime context you cannot verify from the diff and file contents.\n\n\
        Return your findings as a JSON array. For each finding include:\n\
        - \"file\": the file path (\"{}\")\n\
        - \"line\": the line number in the new file (must be within a diff hunk)\n\
        - \"end_line\": (optional) the last line of the affected range, for multi-line issues\n\
        - \"severity\": MUST be exactly one of: \"error\", \"warning\", \"info\"\n\
        - \"title\": a concise summary (10 words or fewer)\n\
        - \"message\": 1–2 sentences on what is specifically wrong in this code. Be direct — name the symbol, state the consequence. Skip general background the reader already knows from the title.\n\
        - \"suggestion\": (optional) the concrete fix — lead with corrected code or a specific action, not a general explanation. Don't just say \"consider fixing this\".\n\
        - \"agent\": \"{}\"\n\n\
        Be concise. The title already states the issue category — the message should add *specific* \
        detail (which symbol, what happens), not restate the title in longer form. \
        Assume the reader is a competent developer who does not need general background explanations.\n\n\
        Severity definitions:\n\
        - \"error\": confirmed bug or vulnerability that will cause incorrect behavior or a security breach\n\
        - \"warning\": likely issue or significant code smell that should be addressed\n\
        - \"info\": suggestion, minor improvement, or observation worth noting\n\n\
        IMPORTANT: The \"severity\" field must be one of \"error\", \"warning\", or \"info\". \
        Do NOT use values like \"critical\", \"major\", \"minor\", \"high\", or \"low\".\n\n\
        Example finding:\n\
        ```json\n\
        {{\n\
          \"file\": \"src/handler.rs\",\n\
          \"line\": 42,\n\
          \"end_line\": 45,\n\
          \"severity\": \"error\",\n\
          \"title\": \"Unhandled error from file I/O\",\n\
          \"message\": \"`read_config` panics on missing/unreadable files instead of propagating the error.\",\n\
          \"suggestion\": \"Replace `.unwrap()` with `.map_err(|e| AppError::ConfigLoad(e))?`\",\n\
          \"agent\": \"{}\"\n\
        }}\n\
        ```\n\n\
        If there are no issues, return an empty array: []\n",
        agent.profile.name,
        agent.profile.description,
        file_path,
        agent.profile.name,
        agent.profile.name,
    ));

    prompt
}

/// Build a coordination note listing sibling reviewers and their focus areas.
///
/// When multiple agents are active, this tells the current reviewer what the
/// other reviewers cover so it can avoid duplicating their work. Uses each
/// profile's tags to summarise focus areas.
fn build_coordination_note(current: &AgentDefinition, all_agents: &[AgentDefinition]) -> String {
    let others: Vec<String> = all_agents
        .iter()
        .filter(|a| a.profile.name != current.profile.name)
        .map(|a| {
            if a.profile.tags.is_empty() {
                format!("**{}** ({})", a.profile.name, a.profile.description)
            } else {
                format!(
                    "**{}** (focuses on: {})",
                    a.profile.name,
                    a.profile.tags.join(", ")
                )
            }
        })
        .collect();

    if others.is_empty() {
        String::new()
    } else {
        format!(
            "You are one of several specialized reviewers running in parallel. \
             The other active reviewers are: {}. \
             Stay in your lane — avoid duplicating findings that fall squarely \
             in another reviewer's focus area.\n\n",
            others.join("; ")
        )
    }
}

/// Build the agentic context section for the user prompt.
///
/// Provides the LLM with:
/// - A snapshot of the repository root directory listing
/// - A list of all files changed in this review (for cross-referencing)
/// - Guidance on using tools with relative paths
/// - Encouragement to explore before concluding
fn build_agentic_context(
    current_diff: &FileDiff,
    context: &ReviewContext,
    agent: &AgentDefinition,
) -> String {
    let mut section = String::new();

    // Embed a snapshot of the repo root so the LLM knows the project layout
    // without needing to make a speculative list_directory tool call.
    if let Ok(entries) = list_repo_root(&context.repo_root) {
        section.push_str("## Repository Structure\n\n");
        section.push_str("The following files and directories are at the repository root:\n\n");
        section.push_str("```\n");
        for entry in &entries {
            section.push_str(entry);
            section.push('\n');
        }
        section.push_str("```\n\n");
    }

    // List all changed files so the LLM knows what else to explore
    let other_files: Vec<&str> = context
        .diffs
        .iter()
        .filter(|d| !d.is_binary && d.path() != current_diff.path())
        .map(|d| d.path())
        .collect();

    if !other_files.is_empty() {
        section.push_str("## Other Changed Files in This Review\n\n");
        section.push_str(
            "These files are also part of this review. \
             Use `read_file` to examine them if the current diff references or affects them:\n\n",
        );
        for path in &other_files {
            section.push_str(&format!("- `{path}`\n"));
        }
        section.push('\n');
    }

    // Tool usage guidance with path context
    section.push_str("## Agentic Exploration\n\n");
    section.push_str(
        "You have tools to explore the repository. \
         All file paths must be **relative to the repository root** \
         (e.g., `src/main.rs`, not an absolute path).\n\n",
    );

    section.push_str(
        "**Available tools:**\n\
         - `read_file` — read any file in the repository by relative path\n\
         - `search_text` — search for text patterns (literal or regex) across the codebase\n\
         - `list_directory` — list directory contents (use `.` for the repo root)\n",
    );

    // Mention custom tools if the agent defines any
    for tool in &agent.profile.tools {
        section.push_str(&format!("- `{}` — {}\n", tool.name, tool.description));
    }

    section.push_str(
        "\n**Before reporting findings, use the tools to:**\n\
         - Read imported modules, types, or functions referenced in the diff\n\
         - Search for callers or usages of modified functions/types\n\
         - Check whether tests exist for the changed code\n\
         - Explore the directory structure around the changed file\n\
         - Verify assumptions instead of guessing\n\n",
    );

    section
}

/// Synchronously list the top-level entries in a repo directory.
///
/// Returns a compact formatted list (directories with trailing `/`, files
/// with sizes). Hidden entries (`.git`, etc.) are skipped. Used to embed
/// a project structure snapshot in the agentic user prompt so the LLM
/// doesn't have to make a speculative first tool call.
fn list_repo_root(repo_root: &str) -> Result<Vec<String>, std::io::Error> {
    let root = std::path::Path::new(repo_root);
    let mut entries: Vec<(String, bool, Option<u64>)> = Vec::new();

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/directories
        if name.starts_with('.') {
            continue;
        }

        let metadata = entry.metadata().ok();
        let is_dir = metadata.as_ref().map_or(false, |m| m.is_dir());
        let size = if is_dir {
            None
        } else {
            metadata.map(|m| m.len())
        };

        entries.push((name, is_dir, size));
    }

    // Sort: directories first, then alphabetically
    entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    Ok(entries
        .into_iter()
        .map(|(name, is_dir, size)| {
            if is_dir {
                format!("{name}/")
            } else if let Some(size) = size {
                format!("{name} ({size} bytes)")
            } else {
                name
            }
        })
        .collect())
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

/// Filter findings to only include those within diff hunk boundaries.
///
/// A finding is in scope if:
/// - Its file matches a file in the diffs, AND
/// - Its line number (or range) overlaps with at least one hunk's new-file range.
///
/// This prevents the LLM from reporting pre-existing issues in unchanged code
/// that was provided only as surrounding context.
fn filter_to_diff_scope(findings: Vec<Finding>, diffs: &[FileDiff]) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| finding_in_diff_scope(f, diffs))
        .collect()
}

/// Check whether a single finding falls within any diff hunk for its file.
fn finding_in_diff_scope(finding: &Finding, diffs: &[FileDiff]) -> bool {
    // Find the matching diff for this finding's file
    let Some(diff) = diffs.iter().find(|d| d.path() == finding.file) else {
        // Finding references a file not in the diffs — out of scope
        return false;
    };

    let finding_start = finding.line;
    let finding_end = finding.end_line.unwrap_or(finding.line);

    // Check if the finding overlaps with any hunk's new-file range
    diff.hunks.iter().any(|hunk| {
        let hunk_start = hunk.new_start;
        let hunk_end = hunk
            .new_start
            .saturating_add(hunk.new_count)
            .saturating_sub(1);
        // Overlap check: finding_start <= hunk_end && hunk_start <= finding_end
        finding_start <= hunk_end && hunk_start <= finding_end
    })
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
            is_path_scan: false,
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, &[agent.clone()], None, false);
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
            is_path_scan: false,
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

        let prompt = build_prompt(
            &diff,
            &context,
            &agent,
            &[agent.clone()],
            Some(&prior),
            false,
        );
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
            is_path_scan: false,
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, &[agent.clone()], None, false);
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
            is_path_scan: false,
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

        let base = build_prompt(&diff, &context, &agent, &[agent.clone()], None, false);
        let with_prior = build_prompt_with_prior(&base, &prior);

        // Prior findings section should appear before Instructions
        let prior_pos = with_prior.find("Previous Review Findings").unwrap();
        let instr_pos = with_prior.find("## Instructions").unwrap();
        assert!(prior_pos < instr_pos);
        assert!(with_prior.contains("Critical bug"));
    }

    // --- Diff-scope filtering tests ---

    fn make_diff_with_hunk(path: &str, new_start: u32, new_count: u32) -> FileDiff {
        FileDiff {
            old_path: path.into(),
            new_path: path.into(),
            is_new: false,
            is_deleted: false,
            is_rename: false,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: new_count,
                new_start,
                new_count,
                header: None,
                lines: (0..new_count)
                    .map(|i| DiffLine {
                        line_type: DiffLineType::Added,
                        content: format!("line {}", new_start + i),
                        old_line_no: None,
                        new_line_no: Some(new_start + i),
                    })
                    .collect(),
            }],
        }
    }

    fn make_finding_at(file: &str, line: u32) -> Finding {
        Finding {
            file: file.into(),
            line,
            end_line: None,
            severity: crate::models::finding::Severity::Warning,
            title: "test".into(),
            message: "test".into(),
            suggestion: None,
            agent: "test".into(),
        }
    }

    #[test]
    fn finding_inside_hunk_kept() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let findings = vec![make_finding_at("a.rs", 12)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_outside_hunk_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let findings = vec![make_finding_at("a.rs", 50)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_on_hunk_boundary_start() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let findings = vec![make_finding_at("a.rs", 10)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_on_hunk_boundary_end() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let findings = vec![make_finding_at("a.rs", 14)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_just_before_hunk_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let findings = vec![make_finding_at("a.rs", 9)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_just_after_hunk_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let findings = vec![make_finding_at("a.rs", 15)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_wrong_file_filtered() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)];
        let findings = vec![make_finding_at("b.rs", 12)];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn finding_with_range_overlapping_hunk() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let mut finding = make_finding_at("a.rs", 8);
        finding.end_line = Some(11); // range 8-11 overlaps hunk 10-14
        let result = filter_to_diff_scope(vec![finding], &diffs);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn finding_with_range_not_overlapping_hunk() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let mut finding = make_finding_at("a.rs", 1);
        finding.end_line = Some(5); // range 1-5, hunk 10-14 — no overlap
        let result = filter_to_diff_scope(vec![finding], &diffs);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn mixed_findings_filtered_correctly() {
        let diffs = vec![make_diff_with_hunk("a.rs", 10, 5)]; // lines 10-14
        let findings = vec![
            make_finding_at("a.rs", 10), // in scope
            make_finding_at("a.rs", 50), // out of scope
            make_finding_at("a.rs", 14), // in scope
            make_finding_at("b.rs", 10), // wrong file
        ];
        let result = filter_to_diff_scope(findings, &diffs);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].line, 10);
        assert_eq!(result[1].line, 14);
    }

    #[test]
    fn multiple_hunks_findings_in_second_hunk() {
        let mut diff = make_diff_with_hunk("a.rs", 10, 5); // lines 10-14
        diff.hunks.push(Hunk {
            old_start: 50,
            old_count: 3,
            new_start: 50,
            new_count: 3,
            header: None,
            lines: vec![DiffLine {
                line_type: DiffLineType::Added,
                content: "line 50".into(),
                old_line_no: None,
                new_line_no: Some(50),
            }],
        }); // lines 50-52
        let findings = vec![
            make_finding_at("a.rs", 12), // in hunk 1
            make_finding_at("a.rs", 30), // between hunks — out of scope
            make_finding_at("a.rs", 51), // in hunk 2
        ];
        let result = filter_to_diff_scope(findings, &vec![diff]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].line, 12);
        assert_eq!(result[1].line, 51);
    }

    #[test]
    fn prompt_includes_scope_rule() {
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
            is_path_scan: false,
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, &[agent.clone()], None, false);
        assert!(prompt.contains("IMPORTANT SCOPE RULE"));
        assert!(prompt.contains("do NOT flag pre-existing issues"));
    }

    #[test]
    fn build_prompt_agentic_includes_tool_guidance() {
        let diff = FileDiff {
            old_path: "src/lib.rs".into(),
            new_path: "src/lib.rs".into(),
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
                    content: "use crate::models::Finding;".into(),
                    old_line_no: None,
                    new_line_no: Some(1),
                }],
            }],
        };
        let other_diff = FileDiff {
            old_path: "src/models/finding.rs".into(),
            new_path: "src/models/finding.rs".into(),
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
                    content: "pub struct Finding {}".into(),
                    old_line_no: None,
                    new_line_no: Some(1),
                }],
            }],
        };
        let context = ReviewContext {
            diffs: vec![diff.clone(), other_diff],
            baseline: BaselineContext::default(),
            repo_root: "/tmp".into(),
            is_path_scan: false,
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, &[agent.clone()], None, true);

        // Should include agentic exploration section
        assert!(prompt.contains("Agentic Exploration"));
        assert!(prompt.contains("read_file"));
        assert!(prompt.contains("search_text"));
        assert!(prompt.contains("list_directory"));
        assert!(prompt.contains("relative to the repository root"));

        // Should list the other changed file
        assert!(prompt.contains("src/models/finding.rs"));
        assert!(prompt.contains("Other Changed Files"));
    }

    #[test]
    fn build_prompt_non_agentic_excludes_tool_guidance() {
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
            is_path_scan: false,
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, &[agent.clone()], None, false);

        // Non-agentic should NOT include tool guidance
        assert!(!prompt.contains("Agentic Exploration"));
        assert!(!prompt.contains("Other Changed Files"));
    }

    #[test]
    fn coordination_note_with_multiple_agents() {
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
            is_path_scan: false,
        };
        let backend = crate::agents::builtin::get_builtin("backend").unwrap();
        let security = crate::agents::builtin::get_builtin("security").unwrap();
        let all_agents = vec![backend.clone(), security.clone()];

        let prompt = build_prompt(&diff, &context, &backend, &all_agents, None, false);

        // Should include coordination note mentioning the security reviewer
        assert!(prompt.contains("specialized reviewers running in parallel"));
        assert!(prompt.contains("**security**"));
        // Should list security's tags
        assert!(prompt.contains("auth"));
        assert!(prompt.contains("injection"));
        // The coordination note should NOT list the current agent as a sibling
        let coord_note = build_coordination_note(&backend, &all_agents);
        assert!(!coord_note.contains("**backend**"));
    }

    #[test]
    fn coordination_note_absent_with_single_agent() {
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
            is_path_scan: false,
        };
        let agent = crate::agents::builtin::get_builtin("backend").unwrap();

        let prompt = build_prompt(&diff, &context, &agent, &[agent.clone()], None, false);

        // Single agent should NOT have a coordination note
        assert!(!prompt.contains("specialized reviewers running in parallel"));
    }
}
