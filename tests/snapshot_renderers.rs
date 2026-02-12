//! Snapshot tests for output renderers.
//!
//! Each test renders a standard set of findings through a renderer
//! and compares the output against expected fixture files.

use nitpik::models::finding::{Finding, Severity};
use nitpik::output::bitbucket::BitbucketRenderer;
use nitpik::output::forgejo::ForgejoRenderer;
use nitpik::output::github::GithubRenderer;
use nitpik::output::gitlab::GitlabRenderer;
use nitpik::output::json::JsonRenderer;
use nitpik::output::OutputRenderer;

/// Standard test findings used across all snapshot tests.
fn test_findings() -> Vec<Finding> {
    vec![
        Finding {
            file: "src/main.rs".into(),
            line: 42,
            end_line: None,
            severity: Severity::Error,
            title: "Unwrap in production code".into(),
            message: "Using .unwrap() can cause a panic at runtime. Use proper error handling with ? or .expect().".into(),
            suggestion: Some("Replace .unwrap() with .context(\"description\")? using anyhow".into()),
            agent: "backend".into(),
        },
        Finding {
            file: "src/main.rs".into(),
            line: 87,
            end_line: None,
            severity: Severity::Warning,
            title: "Missing error context".into(),
            message: "This error propagation loses context about what operation failed.".into(),
            suggestion: None,
            agent: "backend".into(),
        },
        Finding {
            file: "src/utils.rs".into(),
            line: 15,
            end_line: Some(20),
            severity: Severity::Info,
            title: "Consider extracting helper".into(),
            message: "This block of logic is repeated in multiple places and could be extracted into a shared helper function.".into(),
            suggestion: Some("Create a `validate_input()` function in utils.rs".into()),
            agent: "architect".into(),
        },
    ]
}

#[test]
fn snapshot_json_renderer() {
    let renderer = JsonRenderer;
    let output = renderer.render(&test_findings());

    let actual: serde_json::Value = serde_json::from_str(&output).unwrap();
    let expected_str =
        std::fs::read_to_string("tests/fixtures/expected_json_output.json").unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected_str).unwrap();

    assert_eq!(
        actual, expected,
        "JSON renderer output does not match snapshot.\nActual:\n{output}"
    );
}

#[test]
fn snapshot_github_renderer() {
    let renderer = GithubRenderer;
    let output = renderer.render(&test_findings());

    let expected = std::fs::read_to_string("tests/fixtures/expected_github_output.txt").unwrap();

    assert_eq!(
        output, expected,
        "GitHub renderer output does not match snapshot.\nActual:\n{output}"
    );
}

#[test]
fn snapshot_bitbucket_renderer() {
    let renderer = BitbucketRenderer;
    let output = renderer.render(&test_findings());

    let actual: serde_json::Value = serde_json::from_str(&output).unwrap();
    let expected_str =
        std::fs::read_to_string("tests/fixtures/expected_bitbucket_output.json").unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected_str).unwrap();

    assert_eq!(
        actual, expected,
        "Bitbucket renderer output does not match snapshot.\nActual:\n{output}"
    );
}

#[test]
fn json_renderer_empty_findings() {
    let renderer = JsonRenderer;
    let output = renderer.render(&[]);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["findings"].as_array().unwrap().len(), 0);
    assert_eq!(parsed["summary"]["total"], 0);
}

#[test]
fn github_renderer_empty_findings() {
    let renderer = GithubRenderer;
    let output = renderer.render(&[]);
    assert_eq!(output, "");
}

#[test]
fn bitbucket_renderer_empty_findings() {
    let renderer = BitbucketRenderer;
    let output = renderer.render(&[]);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["annotations"].as_array().unwrap().len(), 0);
}

#[test]
fn snapshot_gitlab_renderer() {
    let renderer = GitlabRenderer;
    let output = renderer.render(&test_findings());

    let actual: serde_json::Value = serde_json::from_str(&output).unwrap();
    let expected_str =
        std::fs::read_to_string("tests/fixtures/expected_gitlab_output.json").unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected_str).unwrap();

    assert_eq!(
        actual, expected,
        "GitLab renderer output does not match snapshot.\nActual:\n{output}"
    );
}

#[test]
fn gitlab_renderer_empty_findings() {
    let renderer = GitlabRenderer;
    let output = renderer.render(&[]);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap();
    assert!(parsed.is_empty());
}

/// Generate the GitLab fixture file from current renderer output.
///
/// Run with: `cargo test generate_gitlab_fixture -- --ignored`
#[test]
#[ignore]
fn generate_gitlab_fixture() {
    let renderer = GitlabRenderer;
    let output = renderer.render(&test_findings());
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    let pretty = serde_json::to_string_pretty(&parsed).unwrap();
    std::fs::write("tests/fixtures/expected_gitlab_output.json", pretty + "\n").unwrap();
    eprintln!("Wrote tests/fixtures/expected_gitlab_output.json");
}

#[test]
fn snapshot_forgejo_renderer() {
    let renderer = ForgejoRenderer;
    let output = renderer.render(&test_findings());

    let actual: serde_json::Value = serde_json::from_str(&output).unwrap();
    let expected_str =
        std::fs::read_to_string("tests/fixtures/expected_forgejo_output.json").unwrap();
    let expected: serde_json::Value = serde_json::from_str(&expected_str).unwrap();

    assert_eq!(
        actual, expected,
        "Forgejo renderer output does not match snapshot.\nActual:\n{output}"
    );
}

#[test]
fn forgejo_renderer_empty_findings() {
    let renderer = ForgejoRenderer;
    let output = renderer.render(&[]);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["event"], "COMMENT");
    assert!(parsed["body"].as_str().unwrap().contains("0 findings"));
    assert_eq!(parsed["comments"].as_array().unwrap().len(), 0);
}

/// Generate the Forgejo fixture file from current renderer output.
///
/// Run with: `cargo test generate_forgejo_fixture -- --ignored`
#[test]
#[ignore]
fn generate_forgejo_fixture() {
    let renderer = ForgejoRenderer;
    let output = renderer.render(&test_findings());
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    let pretty = serde_json::to_string_pretty(&parsed).unwrap();
    std::fs::write("tests/fixtures/expected_forgejo_output.json", pretty + "\n").unwrap();
    eprintln!("Wrote tests/fixtures/expected_forgejo_output.json");
}
