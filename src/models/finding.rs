//! Finding types representing review results.

use clap::ValueEnum;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Severity level of a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, ValueEnum, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Informational suggestion.
    Info,
    /// Potential issue that should be addressed.
    Warning,
    /// Critical issue that must be fixed.
    Error,
}

/// Custom deserializer for Severity that accepts common LLM variations.
///
/// LLMs sometimes return severity values like "Critical", "Major", "Minor",
/// "High", "Medium", "Low", "Note" instead of the expected "error",
/// "warning", "info". This normalizes them.
impl<'de> Deserialize<'de> for Severity {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "info" | "note" | "suggestion" | "low" | "minor" | "trivial" | "style"
                => Ok(Severity::Info),
            "warning" | "warn" | "medium" | "moderate" | "major"
                => Ok(Severity::Warning),
            "error" | "critical" | "high" | "severe" | "blocker" | "fatal"
                => Ok(Severity::Error),
            _ => {
                // Fall back to warning for unrecognised severities rather than failing
                Ok(Severity::Warning)
            }
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "info" => Ok(Severity::Info),
            "warning" => Ok(Severity::Warning),
            "error" => Ok(Severity::Error),
            _ => Err(format!("unknown severity: {s}")),
        }
    }
}

/// A single finding produced by a reviewer agent.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    /// The file path relative to the repo root.
    pub file: String,
    /// The starting line number (1-based).
    pub line: u32,
    /// The ending line number (1-based, inclusive). May equal `line`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// The severity of the finding.
    pub severity: Severity,
    /// Short title summarizing the issue.
    pub title: String,
    /// Detailed explanation of the issue.
    pub message: String,
    /// Suggested fix or improvement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// The agent that produced this finding.
    pub agent: String,
}

/// Summary statistics for a review run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub total: usize,
    pub errors: usize,
    pub warnings: usize,
    pub info: usize,
}

impl Summary {
    /// Compute summary from a list of findings.
    pub fn from_findings(findings: &[Finding]) -> Self {
        let mut s = Summary::default();
        for f in findings {
            s.total += 1;
            match f.severity {
                Severity::Error => s.errors += 1,
                Severity::Warning => s.warnings += 1,
                Severity::Info => s.info += 1,
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
    }

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Info.to_string(), "info");
        assert_eq!(Severity::Warning.to_string(), "warning");
        assert_eq!(Severity::Error.to_string(), "error");
    }

    #[test]
    fn severity_from_str() {
        assert_eq!("info".parse::<Severity>(), Ok(Severity::Info));
        assert_eq!("WARNING".parse::<Severity>(), Ok(Severity::Warning));
        assert_eq!("Error".parse::<Severity>(), Ok(Severity::Error));
        assert!("unknown".parse::<Severity>().is_err());
    }

    #[test]
    fn summary_from_findings() {
        let findings = vec![
            Finding {
                file: "a.rs".into(),
                line: 1,
                end_line: None,
                severity: Severity::Error,
                title: "t".into(),
                message: "m".into(),
                suggestion: None,
                agent: "test".into(),
            },
            Finding {
                file: "b.rs".into(),
                line: 2,
                end_line: None,
                severity: Severity::Warning,
                title: "t".into(),
                message: "m".into(),
                suggestion: None,
                agent: "test".into(),
            },
            Finding {
                file: "c.rs".into(),
                line: 3,
                end_line: None,
                severity: Severity::Info,
                title: "t".into(),
                message: "m".into(),
                suggestion: None,
                agent: "test".into(),
            },
        ];
        let s = Summary::from_findings(&findings);
        assert_eq!(s.total, 3);
        assert_eq!(s.errors, 1);
        assert_eq!(s.warnings, 1);
        assert_eq!(s.info, 1);
    }
}
