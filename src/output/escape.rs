//! Text escaping utilities for output renderers.

/// Escape special characters for XML attribute values (Checkstyle format).
pub fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape special characters for GitHub Actions workflow command parameters.
pub fn github_annotation(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\n', "%0A")
        .replace('\r', "%0D")
}
