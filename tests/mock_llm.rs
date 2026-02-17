//! Integration test using a mock LLM provider.
//!
//! Validates the orchestrator pipeline end-to-end without making
//! real API calls by using a mock implementation of ReviewProvider.

use std::sync::Arc;

use async_trait::async_trait;

use nitpik::cache::CacheEngine;
use nitpik::config::Config;
use nitpik::models::agent::{AgentDefinition, AgentProfile};
use nitpik::models::context::{BaselineContext, ReviewContext};
use nitpik::models::diff::{DiffLine, DiffLineType, FileDiff, Hunk};
use nitpik::models::finding::{Finding, Severity};
use nitpik::orchestrator::ReviewOrchestrator;
use nitpik::progress::ProgressTracker;
use nitpik::providers::{ProviderError, ReviewProvider};

/// A mock review provider that returns canned findings.
struct MockProvider {
    /// The findings to return for every review call.
    canned_findings: Vec<Finding>,
}

impl MockProvider {
    fn new(findings: Vec<Finding>) -> Self {
        Self {
            canned_findings: findings,
        }
    }

    /// A provider that returns no findings.
    fn empty() -> Self {
        Self::new(vec![])
    }
}

#[async_trait]
impl ReviewProvider for MockProvider {
    async fn review(
        &self,
        _agent: &AgentDefinition,
        _prompt: &str,
        _agentic: bool,
        _max_turns: usize,
        _max_tool_calls: usize,
    ) -> Result<Vec<Finding>, ProviderError> {
        Ok(self.canned_findings.clone())
    }
}

/// Helper: build a simple file diff for testing.
fn test_diff(path: &str, added_content: &str) -> FileDiff {
    FileDiff {
        old_path: path.to_string(),
        new_path: path.to_string(),
        is_new: false,
        is_deleted: false,
        is_rename: false,
        is_binary: false,
        hunks: vec![Hunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 2,
            header: None,
            lines: vec![
                DiffLine {
                    line_type: DiffLineType::Context,
                    content: "// existing".to_string(),
                    old_line_no: Some(1),
                    new_line_no: Some(1),
                },
                DiffLine {
                    line_type: DiffLineType::Added,
                    content: added_content.to_string(),
                    old_line_no: None,
                    new_line_no: Some(2),
                },
            ],
        }],
    }
}

/// Helper: build a test agent definition.
fn test_agent(name: &str) -> AgentDefinition {
    AgentDefinition {
        profile: AgentProfile {
            name: name.to_string(),
            description: format!("Test agent: {name}"),
            model: None,
            tags: vec![],
            tools: vec![],
            agentic_instructions: None,
            environment: vec![],
        },
        system_prompt: "You are a test reviewer.".to_string(),
    }
}

/// Helper: build test findings.
fn test_findings(file: &str, agent: &str) -> Vec<Finding> {
    vec![
        Finding {
            file: file.to_string(),
            line: 2,
            end_line: None,
            severity: Severity::Warning,
            title: "Unused variable".to_string(),
            message: "The variable `x` is never used.".to_string(),
            suggestion: Some("Remove the variable or prefix with underscore.".to_string()),
            agent: agent.to_string(),
        },
        Finding {
            file: file.to_string(),
            line: 2,
            end_line: None,
            severity: Severity::Info,
            title: "Consider documentation".to_string(),
            message: "This function lacks a doc comment.".to_string(),
            suggestion: None,
            agent: agent.to_string(),
        },
    ]
}

#[tokio::test]
async fn orchestrator_returns_findings_from_mock_provider() {
    let findings = test_findings("src/main.rs", "test-agent");
    let provider = Arc::new(MockProvider::new(findings.clone()));
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(&["src/main.rs".to_string()], &["test-agent".to_string()], false));
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress, false, None, String::new());

    let context = ReviewContext {
        diffs: vec![test_diff("src/main.rs", "let x = 42;")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };

    let agents = vec![test_agent("test-agent")];
    let result = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("orchestrator should succeed");

    assert_eq!(result.findings.len(), 2);
    assert_eq!(result.findings[0].severity, Severity::Warning);
    assert_eq!(result.findings[1].severity, Severity::Info);
    assert_eq!(result.findings[0].file, "src/main.rs");
    assert_eq!(result.findings[0].agent, "test-agent");
    assert_eq!(result.failed_tasks, 0);
}

#[tokio::test]
async fn orchestrator_returns_empty_for_no_issues() {
    let provider = Arc::new(MockProvider::empty());
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(&["src/lib.rs".to_string()], &["clean-reviewer".to_string()], false));
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress, false, None, String::new());

    let context = ReviewContext {
        diffs: vec![test_diff("src/lib.rs", "fn hello() {}")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };

    let agents = vec![test_agent("clean-reviewer")];
    let result = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("orchestrator should succeed");

    assert!(result.findings.is_empty());
    assert_eq!(result.failed_tasks, 0);
}

#[tokio::test]
async fn orchestrator_errors_on_empty_diffs() {
    let provider = Arc::new(MockProvider::empty());
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(&[], &["any-agent".to_string()], false));
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress, false, None, String::new());

    let context = ReviewContext {
        diffs: vec![],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };

    let agents = vec![test_agent("any-agent")];
    let result = orchestrator.run(&context, &agents, 4, false, 10, 50).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("no diffs"));
}

#[tokio::test]
async fn orchestrator_skips_binary_files() {
    let findings = test_findings("image.png", "test-agent");
    let provider = Arc::new(MockProvider::new(findings));
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(&["image.png".to_string()], &["test-agent".to_string()], false));
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress, false, None, String::new());

    let mut binary_diff = test_diff("image.png", "");
    binary_diff.is_binary = true;

    let context = ReviewContext {
        diffs: vec![binary_diff],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };

    let agents = vec![test_agent("test-agent")];
    let result = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("orchestrator should succeed");

    // Binary files are skipped, so no findings despite the mock returning some
    assert!(result.findings.is_empty());
}

#[tokio::test]
async fn orchestrator_handles_multiple_agents_and_files() {
    let findings_a = vec![Finding {
        file: "a.rs".to_string(),
        line: 1,
        end_line: None,
        severity: Severity::Error,
        title: "From agent-a".to_string(),
        message: "Error found by agent A.".to_string(),
        suggestion: None,
        agent: "agent-a".to_string(),
    }];
    let provider = Arc::new(MockProvider::new(findings_a));
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(&["a.rs".to_string(), "b.rs".to_string()], &["agent-a".to_string(), "agent-b".to_string()], false));
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress, false, None, String::new());

    let context = ReviewContext {
        diffs: vec![
            test_diff("a.rs", "let a = 1;"),
            test_diff("b.rs", "let b = 2;"),
        ],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };

    // Two agents, two files = 4 combinations
    let agents = vec![test_agent("agent-a"), test_agent("agent-b")];
    let result = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("orchestrator should succeed");

    // MockProvider returns same findings for every call
    // 2 agents × 2 files = 4 calls × 1 finding each = 4 findings
    // But deduplication will collapse identical findings, so we get at most 4
    assert!(!result.findings.is_empty());
    // All findings should be errors from the mock
    assert!(result.findings.iter().all(|f| f.severity == Severity::Error));
}

/// Mock provider that always returns an error.
struct FailingProvider;

#[async_trait]
impl ReviewProvider for FailingProvider {
    async fn review(
        &self,
        _agent: &AgentDefinition,
        _prompt: &str,
        _agentic: bool,
        _max_turns: usize,
        _max_tool_calls: usize,
    ) -> Result<Vec<Finding>, ProviderError> {
        Err(ProviderError::ApiError("mock API failure".to_string()))
    }
}

#[tokio::test]
async fn orchestrator_handles_provider_errors_gracefully() {
    let provider = Arc::new(FailingProvider);
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(&["src/main.rs".to_string()], &["failing-agent".to_string()], false));
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress, false, None, String::new());

    let context = ReviewContext {
        diffs: vec![test_diff("src/main.rs", "let x = 1;")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };

    let agents = vec![test_agent("failing-agent")];
    // Orchestrator should succeed but report the failure via failed_tasks
    let result = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("orchestrator should succeed even when provider fails");

    assert!(result.findings.is_empty());
    assert!(result.failed_tasks > 0, "should report failed tasks");
}

#[tokio::test]
async fn cache_prevents_duplicate_calls() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingProvider {
        call_count: AtomicUsize,
        findings: Vec<Finding>,
    }

    #[async_trait]
    impl ReviewProvider for CountingProvider {
        async fn review(
            &self,
            _agent: &AgentDefinition,
            _prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> Result<Vec<Finding>, ProviderError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.findings.clone())
        }
    }

    let findings = test_findings("src/main.rs", "cache-agent");
    let provider = Arc::new(CountingProvider {
        call_count: AtomicUsize::new(0),
        findings: findings.clone(),
    });
    let config = Config::default();
    let cache = CacheEngine::new(true);
    let provider_trait: Arc<dyn ReviewProvider> = Arc::clone(&provider) as Arc<dyn ReviewProvider>;
    let progress = Arc::new(ProgressTracker::new(&["src/main.rs".to_string()], &["cache-agent".to_string()], false));
    let orchestrator = ReviewOrchestrator::new(provider_trait, &config, cache, progress, false, None, String::new());

    // Use unique content to avoid collisions with previously cached results
    let unique_content = format!("let unique_{} = true;", std::process::id());
    let context = ReviewContext {
        diffs: vec![test_diff("src/main.rs", &unique_content)],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };
    let agents = vec![test_agent("cache-agent")];

    // First run — cache miss, should call provider
    let result1 = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("first run should succeed");
    assert_eq!(result1.findings.len(), 2);
    let calls_after_first = provider.call_count.load(Ordering::SeqCst);
    assert_eq!(calls_after_first, 1);

    // Second run with same inputs — cache hit, should NOT call provider
    let result2 = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("second run should succeed");
    assert_eq!(result2.findings.len(), 2);
    // Call count should still be 1 because cache was used
    assert_eq!(provider.call_count.load(Ordering::SeqCst), 1);
}

/// Verifies the sidecar-based prior-findings injection flow:
///
/// 1. First run: provider returns findings, cache + sidecar are written.
/// 2. Second run (different diff): cache misses, prior findings are injected
///    into the prompt, which the mock provider captures and reflects back.
#[tokio::test]
async fn prior_findings_injected_on_cache_invalidation() {
    use std::sync::{atomic::{AtomicUsize, Ordering}, Mutex};

    /// A provider that records every prompt it receives and returns
    /// different findings depending on whether prior findings were
    /// present in the prompt.
    struct PromptCapturingProvider {
        call_count: AtomicUsize,
        captured_prompts: Mutex<Vec<String>>,
        /// Findings to return on the first call (no prior context).
        initial_findings: Vec<Finding>,
        /// Findings to return when prior context is detected.
        followup_findings: Vec<Finding>,
    }

    #[async_trait]
    impl ReviewProvider for PromptCapturingProvider {
        async fn review(
            &self,
            _agent: &AgentDefinition,
            prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> Result<Vec<Finding>, ProviderError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.captured_prompts
                .lock()
                .unwrap()
                .push(prompt.to_string());

            if prompt.contains("Previous Review Findings") {
                Ok(self.followup_findings.clone())
            } else {
                Ok(self.initial_findings.clone())
            }
        }
    }

    // Initial findings (from "first review")
    let initial_findings = vec![Finding {
        file: "src/app.rs".to_string(),
        line: 2,
        end_line: None,
        severity: Severity::Warning,
        title: "Potential null deref".to_string(),
        message: "Could panic at runtime.".to_string(),
        suggestion: Some("Add a None check.".to_string()),
        agent: "prior-agent".to_string(),
    }];

    // Follow-up findings (the model's response when it sees prior context)
    // Line must be within the hunk range (1-2) to survive diff-scope filtering.
    let followup_findings = vec![Finding {
        file: "src/app.rs".to_string(),
        line: 2,
        end_line: None,
        severity: Severity::Info,
        title: "Prior issue resolved, new style nit".to_string(),
        message: "The previous null deref was fixed but naming could improve.".to_string(),
        suggestion: None,
        agent: "prior-agent".to_string(),
    }];

    let provider = Arc::new(PromptCapturingProvider {
        call_count: AtomicUsize::new(0),
        captured_prompts: Mutex::new(Vec::new()),
        initial_findings: initial_findings.clone(),
        followup_findings: followup_findings.clone(),
    });

    let config = Config::default();

    // Use a real on-disk cache (with a temp dir) so sidecars are written
    let cache_dir = tempfile::tempdir().expect("failed to create temp cache dir");
    let store = nitpik::cache::store::FileStore::new_with_dir(cache_dir.path().to_path_buf());

    // --- Run 1: original diff → cache miss → initial findings ---
    let original_content = format!("let original_{} = true;", std::process::id());
    let diff1 = test_diff("src/app.rs", &original_content);
    let _context1 = ReviewContext {
        diffs: vec![diff1],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };
    let agents = vec![test_agent("prior-agent")];

    // Build the same prompt the orchestrator would build, to compute the cache key
    // and pre-populate the store with the sidecar manually, since CacheEngine
    // hides its FileStore. Instead, let's use the orchestrator with cache enabled
    // by constructing a CacheEngine that points to our temp dir.

    // We need to use CacheEngine + orchestrator for the full flow.
    // Unfortunately CacheEngine::new() creates its own FileStore with a fixed dir.
    // So let's use a raw FileStore + manual cache key computation to simulate
    // run 1, then use the orchestrator for run 2.

    // Simulate run 1 manually via the store:
    let prompt1 = format!(
        "## Diff for: src/app.rs\n\n```diff\n@@ -1,1 +1,2 @@\n // existing\n+{original_content}\n```\n\n\
         ## Instructions\n\nReview the diff above for file `src/app.rs`. \
         You are the **prior-agent** reviewer: Test agent: prior-agent\n\n"
    );
    let cache_key1 = nitpik::cache::cache_key(&prompt1, "prior-agent", &config.provider.model);

    // Store findings + sidecar for run 1
    store.put(&cache_key1, &initial_findings);
    store.put_sidecar("src/app.rs", "prior-agent", &config.provider.model, &cache_key1, "");

    // Verify the sidecar was written
    let prior = store.get_previous("src/app.rs", "prior-agent", &config.provider.model, "different-key", "");
    assert!(prior.is_some(), "sidecar should return prior findings for a different key");
    assert_eq!(prior.unwrap().len(), 1);

    // --- Run 2: changed diff → cache miss → prior findings injected ---
    // Change the diff content so the cache key changes
    let changed_content = format!("let changed_{} = false;", std::process::id());
    let diff2 = test_diff("src/app.rs", &changed_content);
    let context2 = ReviewContext {
        diffs: vec![diff2],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };

    // We need an orchestrator that uses the same cache directory.
    // Since CacheEngine::new() doesn't let us pass a custom dir, we'll
    // build a CacheEngine from a store that uses our temp dir via a
    // workaround: set up environment so dirs::config_dir points here.
    // Instead, let's just verify the prompt capturing works with the
    // orchestrator by using CacheEngine(enabled=false) for the provider call
    // and manually checking that `get_previous` returns the right data.

    // Actually, the simplest approach: create a new orchestrator that has
    // cache disabled but verify the prompt manually.

    // Better approach: verify the sidecar store directly, then test the
    // orchestrator prompt injection by checking captured prompts.

    // The orchestrator uses its own internal cache. For this test, let's
    // verify the two halves independently:
    //
    // Half 1 (already verified above): FileStore sidecar read/write works.
    //
    // Half 2: Verify the orchestrator actually injects prior findings.
    // We'll do this by running the orchestrator with cache enabled but
    // using a brand-new CacheEngine (empty cache), and manually pre-seeding
    // the cache directory.

    // Get the actual cache directory from a real CacheEngine
    let real_cache = CacheEngine::new(true);
    let cache_path = real_cache.path().expect("cache path should exist").clone();

    // Pre-seed the cache: write findings + sidecar under the real cache dir
    let seeded_store = nitpik::cache::store::FileStore::new_with_dir(cache_path.clone());
    seeded_store.put(&cache_key1, &initial_findings);
    seeded_store.put_sidecar(
        "src/app.rs",
        "prior-agent",
        &config.provider.model,
        &cache_key1,
        "",
    );

    // Now create an orchestrator with cache enabled — it will use the same dir
    let cache2 = CacheEngine::new(true);
    let progress = Arc::new(ProgressTracker::new(
        &["src/app.rs".to_string()],
        &["prior-agent".to_string()],
        false,
    ));
    let provider_trait: Arc<dyn ReviewProvider> = Arc::clone(&provider) as Arc<dyn ReviewProvider>;
    let orchestrator = ReviewOrchestrator::new(
        provider_trait,
        &config,
        cache2,
        progress,
        false, // no_prior_context = false → inject prior findings
        None,  // max_prior_findings = unlimited
        String::new(),
    );

    let result2 = orchestrator
        .run(&context2, &agents, 4, false, 10, 50)
        .await
        .expect("second run should succeed");

    // The provider should have been called exactly once (cache miss on new diff)
    assert_eq!(provider.call_count.load(Ordering::SeqCst), 1);

    // The prompt should contain the prior findings section
    let prompts = provider.captured_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1, "provider should have been called once");
    assert!(
        prompts[0].contains("Previous Review Findings"),
        "prompt should contain prior findings section"
    );
    assert!(
        prompts[0].contains("Potential null deref"),
        "prompt should contain the title of the prior finding"
    );
    assert!(
        prompts[0].contains("Re-raise"),
        "prompt should contain instructions about re-raising findings"
    );

    // The result should contain the follow-up findings (from the mock)
    assert_eq!(result2.findings.len(), 1);
    assert_eq!(result2.findings[0].title, "Prior issue resolved, new style nit");

    // Clean up the seeded entries from the real cache dir
    let _ = seeded_store.clear();
}

/// Verifies that `--no-prior-context` suppresses prior findings injection.
#[tokio::test]
async fn no_prior_context_flag_suppresses_injection() {
    use std::sync::{atomic::{AtomicUsize, Ordering}, Mutex};

    struct PromptRecordingProvider {
        call_count: AtomicUsize,
        captured_prompts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ReviewProvider for PromptRecordingProvider {
        async fn review(
            &self,
            _agent: &AgentDefinition,
            prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> Result<Vec<Finding>, ProviderError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.captured_prompts
                .lock()
                .unwrap()
                .push(prompt.to_string());
            Ok(vec![])
        }
    }

    let initial_findings = vec![Finding {
        file: "src/lib.rs".to_string(),
        line: 5,
        end_line: None,
        severity: Severity::Error,
        title: "SQL injection".to_string(),
        message: "User input concatenated into query.".to_string(),
        suggestion: None,
        agent: "sec-agent".to_string(),
    }];

    let config = Config::default();

    // Pre-seed the real cache with findings + sidecar
    let real_cache = CacheEngine::new(true);
    let cache_path = real_cache.path().expect("cache path").clone();
    let seeded_store = nitpik::cache::store::FileStore::new_with_dir(cache_path.clone());

    let seed_content = format!("let seed_npc_{} = 1;", std::process::id());
    let seed_prompt = format!("seed-prompt-{seed_content}");
    let seed_key = nitpik::cache::cache_key(&seed_prompt, "sec-agent", &config.provider.model);
    seeded_store.put(&seed_key, &initial_findings);
    seeded_store.put_sidecar("src/lib.rs", "sec-agent", &config.provider.model, &seed_key, "");

    // Run orchestrator with no_prior_context = true
    let provider = Arc::new(PromptRecordingProvider {
        call_count: AtomicUsize::new(0),
        captured_prompts: Mutex::new(Vec::new()),
    });
    let cache = CacheEngine::new(true);
    let progress = Arc::new(ProgressTracker::new(
        &["src/lib.rs".to_string()],
        &["sec-agent".to_string()],
        false,
    ));
    let provider_trait: Arc<dyn ReviewProvider> = Arc::clone(&provider) as Arc<dyn ReviewProvider>;
    let orchestrator = ReviewOrchestrator::new(
        provider_trait,
        &config,
        cache,
        progress,
        true, // no_prior_context = true → suppress prior findings
        None,
        String::new(),
    );

    let new_content = format!("let new_npc_{} = 2;", std::process::id());
    let context = ReviewContext {
        diffs: vec![test_diff("src/lib.rs", &new_content)],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };
    let agents = vec![test_agent("sec-agent")];

    let _result = orchestrator
        .run(&context, &agents, 4, false, 10, 50)
        .await
        .expect("run should succeed");

    // The prompt should NOT contain prior findings
    let prompts = provider.captured_prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(
        !prompts[0].contains("Previous Review Findings"),
        "prompt should NOT contain prior findings when --no-prior-context is set"
    );
    assert!(
        !prompts[0].contains("SQL injection"),
        "prompt should NOT mention prior finding titles"
    );

    // Clean up
    let _ = seeded_store.clear();
}

// ---------------------------------------------------------------------------
// Custom tool integration tests
// ---------------------------------------------------------------------------

/// Helper: build a test agent definition with custom tools.
fn test_agent_with_tools(name: &str, tools: Vec<nitpik::models::agent::CustomToolDefinition>) -> AgentDefinition {
    AgentDefinition {
        profile: AgentProfile {
            name: name.to_string(),
            description: format!("Test agent with tools: {name}"),
            model: None,
            tags: vec![],
            tools,
            agentic_instructions: None,
            environment: vec![],
        },
        system_prompt: "You are a test reviewer with tools.".to_string(),
    }
}

/// Verifies that custom tools defined in an agent profile appear in the
/// user prompt's agentic exploration section when agentic mode is enabled.
#[tokio::test]
async fn custom_tools_appear_in_agentic_prompt() {
    use std::sync::Mutex;
    use nitpik::models::agent::{CustomToolDefinition, ToolParameter};

    struct PromptCapture {
        prompts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ReviewProvider for PromptCapture {
        async fn review(
            &self,
            _agent: &AgentDefinition,
            prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> Result<Vec<Finding>, ProviderError> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            Ok(vec![Finding {
                file: "src/app.rs".to_string(),
                line: 2,
                end_line: None,
                severity: Severity::Info,
                title: "Style nit".to_string(),
                message: "Minor style issue.".to_string(),
                suggestion: None,
                agent: "tool-agent".to_string(),
            }])
        }
    }

    let tools = vec![
        CustomToolDefinition {
            name: "run_tests".to_string(),
            description: "Run the project test suite".to_string(),
            command: "cargo test".to_string(),
            parameters: vec![ToolParameter {
                name: "filter".to_string(),
                param_type: "string".to_string(),
                description: "Test name filter".to_string(),
                required: false,
            }],
        },
        CustomToolDefinition {
            name: "lint".to_string(),
            description: "Run the linter".to_string(),
            command: "cargo clippy".to_string(),
            parameters: vec![],
        },
    ];

    let provider = Arc::new(PromptCapture {
        prompts: Mutex::new(Vec::new()),
    });
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(
        &["src/app.rs".to_string()],
        &["tool-agent".to_string()],
        false,
    ));
    let provider_trait: Arc<dyn ReviewProvider> = Arc::clone(&provider) as Arc<dyn ReviewProvider>;
    let orchestrator = ReviewOrchestrator::new(
        provider_trait, &config, cache, progress, false, None, String::new(),
    );

    let context = ReviewContext {
        diffs: vec![test_diff("src/app.rs", "let x = 42;")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };
    let agents = vec![test_agent_with_tools("tool-agent", tools)];

    // Run with agentic=true
    let _result = orchestrator
        .run(&context, &agents, 4, true, 5, 10)
        .await
        .expect("orchestrator should succeed");

    let prompts = provider.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1, "provider should have been called once");

    let prompt = &prompts[0];

    // The user prompt's agentic section should list custom tools
    assert!(
        prompt.contains("`run_tests`"),
        "prompt should mention the run_tests custom tool"
    );
    assert!(
        prompt.contains("`lint`"),
        "prompt should mention the lint custom tool"
    );
    assert!(
        prompt.contains("Run the project test suite"),
        "prompt should contain the run_tests description"
    );
    assert!(
        prompt.contains("Run the linter"),
        "prompt should contain the lint description"
    );
}

/// Verifies that custom tools do NOT appear in the user prompt when
/// agentic mode is disabled (non-agentic review).
#[tokio::test]
async fn custom_tools_absent_in_non_agentic_prompt() {
    use std::sync::Mutex;
    use nitpik::models::agent::{CustomToolDefinition, ToolParameter};

    struct PromptCapture {
        prompts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl ReviewProvider for PromptCapture {
        async fn review(
            &self,
            _agent: &AgentDefinition,
            prompt: &str,
            _agentic: bool,
            _max_turns: usize,
            _max_tool_calls: usize,
        ) -> Result<Vec<Finding>, ProviderError> {
            self.prompts.lock().unwrap().push(prompt.to_string());
            Ok(vec![Finding {
                file: "src/app.rs".to_string(),
                line: 2,
                end_line: None,
                severity: Severity::Info,
                title: "Style nit".to_string(),
                message: "Minor.".to_string(),
                suggestion: None,
                agent: "tool-agent".to_string(),
            }])
        }
    }

    let tools = vec![CustomToolDefinition {
        name: "run_tests".to_string(),
        description: "Run the project test suite".to_string(),
        command: "cargo test".to_string(),
        parameters: vec![ToolParameter {
            name: "filter".to_string(),
            param_type: "string".to_string(),
            description: "Test name filter".to_string(),
            required: false,
        }],
    }];

    let provider = Arc::new(PromptCapture {
        prompts: Mutex::new(Vec::new()),
    });
    let config = Config::default();
    let cache = CacheEngine::new(false);
    let progress = Arc::new(ProgressTracker::new(
        &["src/app.rs".to_string()],
        &["tool-agent".to_string()],
        false,
    ));
    let provider_trait: Arc<dyn ReviewProvider> = Arc::clone(&provider) as Arc<dyn ReviewProvider>;
    let orchestrator = ReviewOrchestrator::new(
        provider_trait, &config, cache, progress, false, None, String::new(),
    );

    let context = ReviewContext {
        diffs: vec![test_diff("src/app.rs", "let y = 99;")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
        is_path_scan: false,
    };
    let agents = vec![test_agent_with_tools("tool-agent", tools)];

    // Run with agentic=false
    let _result = orchestrator
        .run(&context, &agents, 4, false, 5, 10)
        .await
        .expect("orchestrator should succeed");

    let prompts = provider.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 1);

    let prompt = &prompts[0];

    // In non-agentic mode, the agentic exploration section (and custom tools) should be absent
    assert!(
        !prompt.contains("Agentic Exploration"),
        "non-agentic prompt should not contain agentic exploration section"
    );
    assert!(
        !prompt.contains("`run_tests`"),
        "non-agentic prompt should not mention custom tools"
    );
}
