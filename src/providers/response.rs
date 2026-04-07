//! LLM response parsing, error classification, and retry logic.
//!
//! Decoupled from provider construction so these concerns can be tested
//! and reused independently.

use crate::constants::{INITIAL_BACKOFF, MAX_BACKOFF};
use crate::models::finding::Finding;
use crate::providers::ProviderError;
use std::time::Duration;

/// Maximum length of LLM response text to include in parse error messages.
const PARSE_ERROR_PREVIEW_LEN: usize = 2000;

/// Check whether a provider error is transient and worth retrying.
///
/// Matches HTTP status codes commonly used for rate limiting and
/// temporary unavailability: 429 (Too Many Requests), 503 (Service
/// Unavailable), 529 (Overloaded), and connection/timeout errors.
///
/// Parse errors are never retried — the LLM is likely to produce the
/// same malformed output on a retry (especially truncated responses).
pub fn is_retryable(err: &ProviderError) -> bool {
    match err {
        ProviderError::ParseError(_) => false,
        _ => classify_error(err).is_some(),
    }
}

/// Classifies a provider error into a short, user-friendly message.
///
/// Returns `Some(message)` for transient/retryable errors, `None` otherwise.
pub fn classify_error(err: &ProviderError) -> Option<&'static str> {
    match err {
        ProviderError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("429")
                || msg_lower.contains("rate limit")
                || msg_lower.contains("too many requests")
            {
                Some("Rate limited by API")
            } else if msg_lower.contains("503")
                || msg_lower.contains("service unavailable")
                || msg_lower.contains("high demand")
            {
                Some("High model load")
            } else if msg_lower.contains("529") || msg_lower.contains("overloaded") {
                Some("API overloaded")
            } else if msg_lower.contains("502") {
                Some("API gateway error")
            } else if msg_lower.contains("timeout") || msg_lower.contains("timed out") {
                Some("Request timed out")
            } else if msg_lower.contains("connection") {
                Some("Connection error")
            } else if msg_lower.contains("temporarily") || msg_lower.contains("try again") {
                Some("Temporary API error")
            } else {
                None
            }
        }
        ProviderError::ParseError(_) => Some("Failed to parse LLM response"),
        _ => None,
    }
}

/// Compute the backoff duration for a retry attempt using exponential backoff.
pub fn retry_backoff(attempt: u32) -> Duration {
    let backoff = INITIAL_BACKOFF.saturating_mul(2u32.saturating_pow(attempt));
    backoff.min(MAX_BACKOFF)
}

/// Parse the LLM response text into structured findings.
///
/// With `output_schema` enforcing the JSON schema at the provider level,
/// the response is expected to be valid JSON. We still handle an empty
/// response or a `{"findings": [...]}` wrapper gracefully.
///
/// Some providers may return JSON wrapped in markdown code fences
/// (e.g. ```json ... ```), so we extract the inner content first.
pub fn parse_findings_response(response: &str) -> Result<Vec<Finding>, ProviderError> {
    let trimmed = response.trim();

    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    // Try the raw text first, then try extracting from markdown fences
    let candidates = extract_json_candidates(trimmed);

    for candidate in &candidates {
        // Try parsing as a direct array of findings
        if let Ok(findings) = serde_json::from_str::<Vec<Finding>>(candidate) {
            return Ok(trim_finding_fields(findings));
        }

        // Try parsing as {"findings": [...]}
        if let Ok(wrapper) = serde_json::from_str::<serde_json::Value>(candidate) {
            if let Some(findings_arr) = wrapper.get("findings") {
                if let Ok(findings) = serde_json::from_value::<Vec<Finding>>(findings_arr.clone()) {
                    return Ok(trim_finding_fields(findings));
                }
            }
        }
    }

    Err(ProviderError::ParseError(format!(
        "could not parse LLM response as findings JSON. Response: {}",
        &response[..response.len().min(PARSE_ERROR_PREVIEW_LEN)]
    )))
}

/// Trim trailing whitespace from LLM-generated string fields.
///
/// LLMs occasionally include trailing newlines in finding fields, which
/// causes extra blank lines in rendered output.
fn trim_finding_fields(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .map(|mut f| {
            f.title = f.title.trim().to_string();
            f.message = f.message.trim().to_string();
            f.suggestion = f.suggestion.map(|s| s.trim().to_string());
            f
        })
        .collect()
}

/// Regex for extracting content inside markdown code fences.
///
/// The closing ``` must appear at the start of a line (`\n````) to avoid
/// matching triple-backticks embedded inside JSON string values (e.g.
/// suggestion fields containing ```rust code examples).
static FENCE_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)```(?:json)?\s*\n(.*?)\n```").unwrap());

/// Extract candidate JSON strings from a response.
///
/// Returns the trimmed response itself plus any content inside markdown
/// code fences (```json ... ``` or ``` ... ```).
fn extract_json_candidates(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    // First candidate: the raw text
    candidates.push(text.to_string());

    // Second: bracket extraction — find the first '[' and last ']'.
    // This is the most robust strategy when the response contains
    // nested code fences inside JSON string values.
    if let (Some(start), Some(end)) = (text.find('['), text.rfind(']')) {
        if start < end {
            let slice = &text[start..=end];
            candidates.push(slice.to_string());
        }
    }

    // Third: extract content from markdown code fences.
    for cap in FENCE_RE.captures_iter(text) {
        if let Some(inner) = cap.get(1) {
            let inner_trimmed = inner.as_str().trim();
            if !inner_trimmed.is_empty() {
                candidates.push(inner_trimmed.to_string());
            }
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_array() {
        let response = r#"[
            {
                "file": "src/main.rs",
                "line": 42,
                "severity": "error",
                "title": "Bug found",
                "message": "This is a bug",
                "suggestion": "Fix it",
                "agent": "backend"
            }
        ]"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/main.rs");
    }

    #[test]
    fn parse_wrapped_json() {
        let response = r#"{
    "findings": [
        {
            "file": "test.rs",
            "line": 1,
            "severity": "warning",
            "title": "Issue",
            "message": "Problem here",
            "agent": "test"
        }
    ]
}"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn parse_empty_response() {
        let findings = parse_findings_response("").unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_whitespace_only() {
        let findings = parse_findings_response("   \n\n  ").unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_unparseable_response() {
        let result = parse_findings_response("This is random text with no JSON.");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("could not parse"));
    }

    #[test]
    fn parse_empty_json_array() {
        let findings = parse_findings_response("[]").unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_bare_json() {
        let response = r#"[
            {
                "file": "lib.rs",
                "line": 10,
                "severity": "info",
                "title": "Style",
                "message": "Consider renaming",
                "agent": "test"
            }
        ]"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn retryable_429_rate_limit() {
        let err = ProviderError::ApiError(
            "Gemini API error: HttpError: Invalid status code 429 Too Many Requests".into(),
        );
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_503_unavailable() {
        let err = ProviderError::ApiError(
            "Gemini API error: HttpError: Invalid status code 503 Service Unavailable".into(),
        );
        assert!(is_retryable(&err));
    }

    #[test]
    fn retryable_overloaded_message() {
        let err =
            ProviderError::ApiError("Anthropic API error: overloaded — try again later".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn not_retryable_auth_error() {
        let err = ProviderError::ApiError("Invalid API key: 401 Unauthorized".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn not_retryable_parse_error() {
        let err = ProviderError::ParseError("bad json".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn not_retryable_not_configured() {
        let err = ProviderError::NotConfigured("missing key".into());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn backoff_is_exponential() {
        let b0 = retry_backoff(0);
        let b1 = retry_backoff(1);
        let b2 = retry_backoff(2);
        assert_eq!(b0, Duration::from_secs(10));
        assert_eq!(b1, Duration::from_secs(20));
        assert_eq!(b2, Duration::from_secs(40));
    }

    #[test]
    fn backoff_capped_at_max() {
        let b10 = retry_backoff(10);
        assert_eq!(b10, MAX_BACKOFF);
    }

    #[test]
    fn parse_markdown_fenced_json() {
        let response = r#"Here are the findings:
```json
[
    {
        "file": "src/lib.rs",
        "line": 5,
        "severity": "warning",
        "title": "Unused import",
        "message": "This import is unused",
        "agent": "backend"
    }
]
```
"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/lib.rs");
    }

    #[test]
    fn parse_fenced_without_json_label() {
        let response = "Some preamble text\n```\n[]\n```\n";
        let findings = parse_findings_response(response).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn parse_json_embedded_in_prose() {
        let response = r#"I found one issue:
[{"file":"a.rs","line":1,"severity":"info","title":"T","message":"M","agent":"a"}]
That's all."#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn extract_json_candidates_returns_raw_first() {
        let text = r#"[{"a":1}]"#;
        let candidates = extract_json_candidates(text);
        assert!(!candidates.is_empty());
        assert_eq!(candidates[0], text);
    }

    #[test]
    fn extract_json_candidates_no_brackets() {
        let text = "no json here";
        let candidates = extract_json_candidates(text);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], text);
    }

    #[test]
    fn classify_error_502_gateway() {
        let err = ProviderError::ApiError("HTTP 502 Bad Gateway".into());
        assert_eq!(classify_error(&err), Some("API gateway error"));
    }

    #[test]
    fn classify_error_timeout() {
        let err = ProviderError::ApiError("request timed out after 30s".into());
        assert_eq!(classify_error(&err), Some("Request timed out"));
    }

    #[test]
    fn classify_error_connection() {
        let err = ProviderError::ApiError("connection refused".into());
        assert_eq!(classify_error(&err), Some("Connection error"));
    }

    #[test]
    fn classify_error_try_again() {
        let err = ProviderError::ApiError("please try again later".into());
        assert_eq!(classify_error(&err), Some("Temporary API error"));
    }

    #[test]
    fn classify_error_returns_none_for_unknown() {
        let err = ProviderError::ApiError("some unknown error".into());
        assert_eq!(classify_error(&err), None);
    }

    #[test]
    fn classify_error_parse_error() {
        let err = ProviderError::ParseError("could not parse JSON".into());
        assert_eq!(classify_error(&err), Some("Failed to parse LLM response"));
    }

    #[test]
    fn extract_json_candidates_nested_fences() {
        let response = "```json\n[\n  {\n    \"file\": \"db.rs\",\n    \"line\": 10,\n    \"severity\": \"error\",\n    \"title\": \"SQL Injection\",\n    \"message\": \"Vulnerable.\",\n    \"suggestion\": \"Use parameterized queries:\\n```\\nrust\\nquery(?)\\n```\",\n    \"agent\": \"backend\"\n  }\n]\n```";
        let candidates = extract_json_candidates(response);
        let parsed = candidates
            .iter()
            .any(|c| serde_json::from_str::<Vec<Finding>>(c).is_ok());
        assert!(
            parsed,
            "should find a parseable candidate despite nested fences"
        );
    }

    #[test]
    fn parse_findings_wrapper_with_empty_array() {
        let response = r#"{"findings": []}"#;
        let findings = parse_findings_response(response).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn backoff_attempt_zero_equals_initial() {
        assert_eq!(retry_backoff(0), INITIAL_BACKOFF);
    }

    #[test]
    fn retryable_rate_limit_message() {
        let err = ProviderError::ApiError("rate limit exceeded".into());
        assert!(is_retryable(&err));
    }

    #[test]
    fn parse_trims_trailing_newlines_from_fields() {
        let response = r#"[
            {
                "file": "src/main.rs",
                "line": 1,
                "severity": "error",
                "title": "Bug found\n",
                "message": "Description with trailing newline\n",
                "suggestion": "Fix it\n",
                "agent": "test"
            }
        ]"#;
        let findings = parse_findings_response(response).unwrap();
        assert_eq!(findings[0].title, "Bug found");
        assert_eq!(findings[0].message, "Description with trailing newline");
        assert_eq!(findings[0].suggestion.as_deref(), Some("Fix it"));
    }
}
