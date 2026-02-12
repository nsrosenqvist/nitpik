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
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress);

    let context = ReviewContext {
        diffs: vec![test_diff("src/main.rs", "let x = 42;")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
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
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress);

    let context = ReviewContext {
        diffs: vec![test_diff("src/lib.rs", "fn hello() {}")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
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
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress);

    let context = ReviewContext {
        diffs: vec![],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
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
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress);

    let mut binary_diff = test_diff("image.png", "");
    binary_diff.is_binary = true;

    let context = ReviewContext {
        diffs: vec![binary_diff],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
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
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress);

    let context = ReviewContext {
        diffs: vec![
            test_diff("a.rs", "let a = 1;"),
            test_diff("b.rs", "let b = 2;"),
        ],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
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
    let orchestrator = ReviewOrchestrator::new(provider, &config, cache, progress);

    let context = ReviewContext {
        diffs: vec![test_diff("src/main.rs", "let x = 1;")],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
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
    let orchestrator = ReviewOrchestrator::new(provider_trait, &config, cache, progress);

    // Use unique content to avoid collisions with previously cached results
    let unique_content = format!("let unique_{} = true;", std::process::id());
    let context = ReviewContext {
        diffs: vec![test_diff("src/main.rs", &unique_content)],
        baseline: BaselineContext::default(),
        repo_root: "/tmp/test-repo".to_string(),
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
