//! Integration tests for the profiles, validate, and cache CLI commands.
//!
//! These tests exercise the library functions that back each command,
//! using the public API from the nitpik crate.

use nitpik::agents;
use nitpik::agents::parser;
use nitpik::cache::CacheEngine;
use nitpik::models::finding::{Finding, Severity};

// ---------------------------------------------------------------------------
// profiles
// ---------------------------------------------------------------------------

#[tokio::test]
async fn profiles_lists_all_builtins() {
    let agents = agents::list_all_profiles(None).await.unwrap();
    let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
    assert!(names.contains(&"backend"));
    assert!(names.contains(&"frontend"));
    assert!(names.contains(&"architect"));
    assert!(names.contains(&"security"));
}

#[tokio::test]
async fn profiles_includes_custom_from_agent_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ops.md"),
        "---\nname: ops\ndescription: DevOps reviewer\ntags: [ci, infra]\n---\nReview infra code.",
    )
    .unwrap();

    let agents = agents::list_all_profiles(Some(dir.path())).await.unwrap();
    let names: Vec<_> = agents.iter().map(|a| a.profile.name.as_str()).collect();
    assert!(names.contains(&"ops"), "custom profile should appear");
    assert!(names.contains(&"backend"), "built-ins should still appear");
}

#[tokio::test]
async fn profiles_empty_agent_dir() {
    let dir = tempfile::tempdir().unwrap();
    let agents = agents::list_all_profiles(Some(dir.path())).await.unwrap();
    // Only built-ins
    assert_eq!(agents.len(), 4);
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

#[test]
fn validate_valid_profile() {
    let content = r#"---
name: test-agent
description: A test agent for validation
model: gpt-4
tags: [test, validation]
---

You are a test reviewer. Check everything carefully.
"#;
    let agent = parser::parse_agent_definition(content).unwrap();
    assert_eq!(agent.profile.name, "test-agent");
    assert_eq!(agent.profile.description, "A test agent for validation");
    assert_eq!(agent.profile.model, Some("gpt-4".to_string()));
    assert_eq!(agent.profile.tags, vec!["test", "validation"]);
    assert!(!agent.system_prompt.is_empty());
}

#[test]
fn validate_minimal_profile() {
    let content = "---\nname: minimal\ndescription: Bare minimum\n---\nPrompt.";
    let agent = parser::parse_agent_definition(content).unwrap();
    assert_eq!(agent.profile.name, "minimal");
    assert!(agent.profile.model.is_none());
    assert!(agent.profile.tags.is_empty());
}

#[test]
fn validate_missing_frontmatter_delimiters() {
    let result = parser::parse_agent_definition("Just some text without frontmatter");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("frontmatter"), "got: {err}");
}

#[test]
fn validate_missing_name() {
    let content = "---\ndescription: No name field\n---\nPrompt.";
    let result = parser::parse_agent_definition(content);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("name"), "got: {err}");
}

#[test]
fn validate_missing_description() {
    let content = "---\nname: no-desc\n---\nPrompt.";
    let result = parser::parse_agent_definition(content);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("description"), "got: {err}");
}

#[test]
fn validate_unterminated_frontmatter() {
    let content = "---\nname: broken\ndescription: oops\nNo closing delimiter";
    let result = parser::parse_agent_definition(content);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("unterminated") || err.contains("closing"), "got: {err}");
}

#[test]
fn validate_empty_system_prompt() {
    let content = "---\nname: empty-prompt\ndescription: Has no prompt body\n---\n";
    let agent = parser::parse_agent_definition(content).unwrap();
    assert_eq!(agent.profile.name, "empty-prompt");
    // An empty system prompt is valid but the string will be empty
    assert!(agent.system_prompt.is_empty());
}

#[tokio::test]
async fn validate_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("my-agent.md");
    std::fs::write(
        &file,
        "---\nname: file-agent\ndescription: Loaded from disk\ntags: [disk]\n---\nCheck code.",
    )
    .unwrap();

    let content = tokio::fs::read_to_string(&file).await.unwrap();
    let agent = parser::parse_agent_definition(&content).unwrap();
    assert_eq!(agent.profile.name, "file-agent");
    assert_eq!(agent.profile.tags, vec!["disk"]);
}

// ---------------------------------------------------------------------------
// cache
// ---------------------------------------------------------------------------

fn sample_findings() -> Vec<Finding> {
    vec![Finding {
        file: "test.rs".into(),
        line: 10,
        end_line: None,
        severity: Severity::Warning,
        title: "Test".into(),
        message: "A test finding".into(),
        suggestion: None,
        agent: "backend".into(),
    }]
}

#[test]
fn cache_stats_empty() {
    let engine = CacheEngine::new(true);
    // Stats should work even if cache dir doesn't exist yet
    let stats = engine.stats().unwrap();
    // Just verify it doesn't error — the global cache may or may not have entries.
    let _ = stats.entries;
}

#[test]
fn cache_path_returns_value() {
    let engine = CacheEngine::new(true);
    let path = engine.path();
    // On a system with a config dir, this should be Some
    // We just verify no panic and format is sane
    if let Some(p) = path {
        assert!(p.ends_with("cache"));
    }
}

#[test]
fn cache_clear_and_stats_roundtrip() {
    // Use the store directly with a temp dir for isolation
    use nitpik::cache::store::FileStore;

    let dir = tempfile::tempdir().unwrap();
    let store = FileStore::new_with_dir(dir.path().to_path_buf());

    store.put("a", &sample_findings());
    store.put("b", &sample_findings());

    let stats = store.stats().unwrap();
    assert_eq!(stats.entries, 2);
    assert!(stats.total_bytes > 0);

    let cleared = store.clear().unwrap();
    assert_eq!(cleared.entries, 2);

    let after = store.stats().unwrap();
    assert_eq!(after.entries, 0);
    assert_eq!(after.total_bytes, 0);
}

#[test]
fn cache_clear_idempotent() {
    use nitpik::cache::store::FileStore;

    let dir = tempfile::tempdir().unwrap();
    let cache_dir = dir.path().join("cache");
    let store = FileStore::new_with_dir(cache_dir);

    // Clear when nothing exists
    let stats = store.clear().unwrap();
    assert_eq!(stats.entries, 0);

    // Clear again
    let stats = store.clear().unwrap();
    assert_eq!(stats.entries, 0);
}

#[test]
fn cache_stats_human_size() {
    use nitpik::cache::store::CacheStats;

    let small = CacheStats { entries: 1, total_bytes: 42 };
    assert_eq!(small.human_size(), "42 B");

    let kib = CacheStats { entries: 5, total_bytes: 4096 };
    assert_eq!(kib.human_size(), "4.0 KiB");

    let mib = CacheStats { entries: 100, total_bytes: 3 * 1024 * 1024 };
    assert_eq!(mib.human_size(), "3.0 MiB");
}
