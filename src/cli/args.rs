//! Clap argument types, validation, and profile resolution.

use clap::{Parser, ValueEnum};
use std::path::PathBuf;

use nitpik::models::{DEFAULT_PROFILE, Severity};

/// AI-powered code review CLI.
#[derive(Parser, Debug)]
#[command(
    name = "nitpik",
    version = nitpik::constants::VERSION,
    about = super::LICENSE_BANNER_STYLED,
)]
pub struct Cli {
    /// Disable anonymous usage telemetry.
    #[arg(long, global = true, default_value_t = false)]
    pub no_telemetry: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// Available commands.
#[derive(clap::Subcommand, Debug)]
pub enum Command {
    /// Run a code review.
    Review(Box<ReviewArgs>),

    /// List available agent profiles.
    Profiles(ProfilesArgs),

    /// Validate a custom agent profile definition.
    Validate(ValidateArgs),

    /// Manage the result cache.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },

    /// Manage the commercial license key.
    License {
        #[command(subcommand)]
        action: LicenseAction,
    },

    /// Update nitpik to the latest release.
    Update(UpdateArgs),

    /// Print version and build information.
    Version,
}

/// Arguments for the `profiles` subcommand.
#[derive(Parser, Debug)]
pub struct ProfilesArgs {
    /// Directory to scan for additional custom profiles.
    #[arg(long)]
    pub profile_dir: Option<PathBuf>,
}

/// Arguments for the `validate` subcommand.
#[derive(Parser, Debug)]
pub struct ValidateArgs {
    /// Path to the agent profile markdown file to validate.
    pub file: PathBuf,
}

/// Cache management subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum CacheAction {
    /// Remove all cached review results.
    Clear,
    /// Show cache statistics (entry count and size).
    Stats,
    /// Print the cache directory path.
    Path,
}

/// License management subcommands.
#[derive(clap::Subcommand, Debug)]
pub enum LicenseAction {
    /// Store a license key in the global config (~/.config/nitpik/config.toml).
    Activate {
        /// The license key string.
        key: String,
    },
    /// Show current license status (customer, expiry).
    Status,
    /// Remove the license key from the global config.
    Deactivate,
}

/// Arguments for the `update` subcommand.
#[derive(Parser, Debug)]
pub struct UpdateArgs {
    /// Force update even if already on the latest version.
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

/// Arguments for the `review` subcommand.
#[derive(Parser, Debug)]
pub struct ReviewArgs {
    // --- Repo location ---
    /// Path to the repository or working directory (default: current directory).
    #[arg(long, default_value = ".")]
    pub path: PathBuf,

    // --- Input (one required) ---
    /// Pre-computed unified diff file.
    #[arg(long)]
    pub diff_file: Option<PathBuf>,

    /// Read unified diff from stdin.
    #[arg(long, default_value_t = false)]
    pub diff_stdin: bool,

    /// Branch or commit to diff against (uses git diff).
    #[arg(long)]
    pub diff_base: Option<String>,

    /// File or directory to scan directly (review all contents, no git required).
    #[arg(long)]
    pub scan: Option<PathBuf>,

    // --- Profile ---
    /// Comma-separated profiles: built-in names, file paths, or "auto".
    /// Built-in: frontend, backend, architect, security
    #[arg(long, default_value = DEFAULT_PROFILE, value_delimiter = ',')]
    pub profile: Vec<String>,

    /// Directory to resolve bare profile names from.
    #[arg(long)]
    pub profile_dir: Option<PathBuf>,

    /// Select profiles by tag. All profiles (built-in and custom) whose tags
    /// contain any of the given values will be included. Combines with --profile.
    #[arg(long, value_delimiter = ',')]
    pub tag: Vec<String>,

    // --- Output ---
    /// Output format.
    #[arg(long, default_value = "terminal")]
    pub format: OutputFormat,

    /// Exit non-zero if findings meet this severity threshold (default: error).
    #[arg(long)]
    pub fail_on: Option<Severity>,

    /// Never exit non-zero on findings, even when --fail-on or config is set.
    #[arg(long, default_value_t = false, conflicts_with = "fail_on")]
    pub no_fail: bool,

    // --- Agentic ---
    /// Enable agentic context gathering (tools for LLM).
    #[arg(long, default_value_t = false)]
    pub agent: bool,

    /// Max agentic loop turns per file×agent.
    #[arg(long, default_value_t = 10)]
    pub max_turns: usize,

    /// Max tool invocations per file×agent.
    #[arg(long, default_value_t = 10)]
    pub max_tool_calls: usize,

    // --- Security ---
    /// Enable secret detection and redaction before LLM calls.
    ///
    /// Compiling the 200+ built-in rules adds roughly 20-30 seconds of
    /// startup time on the first invocation.
    #[arg(long, default_value_t = false)]
    pub scan_secrets: bool,

    /// Additional gitleaks-format TOML rules file.
    #[arg(long)]
    pub secrets_rules: Option<PathBuf>,

    // --- Performance ---
    /// Max concurrent LLM calls.
    #[arg(long, default_value_t = 5)]
    pub max_concurrent: usize,

    /// Disable result caching.
    #[arg(long, default_value_t = false)]
    pub no_cache: bool,

    /// Disable injection of prior review findings into the prompt on cache invalidation.
    #[arg(long, default_value_t = false)]
    pub no_prior_context: bool,

    /// Maximum number of prior findings to include in the prompt (unlimited by default).
    #[arg(long)]
    pub max_prior_findings: Option<usize>,

    /// Suppress all non-essential output (banner, progress, informational messages).
    /// Only findings and errors are shown.
    #[arg(long, short = 'q', default_value_t = false)]
    pub quiet: bool,

    // --- Context ---
    /// Skip auto-detected project documentation files (AGENTS.md, CONVENTIONS.md, etc.).
    #[arg(long, default_value_t = false)]
    pub no_project_docs: bool,

    /// Skip injecting commit summaries into the review prompt.
    #[arg(long, default_value_t = false)]
    pub no_commit_context: bool,

    /// Comma-separated list of project documentation files to exclude by name.
    /// Example: --exclude-doc AGENTS.md,CONVENTIONS.md
    #[arg(long, value_name = "FILENAME", value_delimiter = ',')]
    pub exclude_doc: Vec<String>,
}

/// Output format options.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Terminal,
    Json,
    Github,
    Gitlab,
    Bitbucket,
    Forgejo,
}

impl OutputFormat {
    /// Render findings using the renderer for this format.
    pub fn render(&self, findings: &[nitpik::models::finding::Finding]) -> String {
        use nitpik::output::OutputRenderer;
        match self {
            OutputFormat::Terminal => nitpik::output::terminal::TerminalRenderer.render(findings),
            OutputFormat::Json => nitpik::output::json::JsonRenderer.render(findings),
            OutputFormat::Github => nitpik::output::github::GithubRenderer.render(findings),
            OutputFormat::Gitlab => nitpik::output::gitlab::GitlabRenderer.render(findings),
            OutputFormat::Bitbucket => {
                nitpik::output::bitbucket::BitbucketRenderer.render(findings)
            }
            OutputFormat::Forgejo => nitpik::output::forgejo::ForgejoRenderer.render(findings),
        }
    }
}

impl ReviewArgs {
    /// Validate that exactly one input source is provided.
    pub fn validate_input(&self) -> Result<InputMode, String> {
        let sources = [
            self.diff_file.is_some(),
            self.diff_stdin,
            self.diff_base.is_some(),
            self.scan.is_some(),
        ];
        let count = sources.iter().filter(|&&x| x).count();

        if count == 0 {
            return Err(
                "one input source is required: --diff-file, --diff-stdin, --diff-base, or --scan"
                    .to_string(),
            );
        }
        if count > 1 {
            return Err(
                "only one input source allowed: --diff-file, --diff-stdin, --diff-base, or --scan"
                    .to_string(),
            );
        }

        if let Some(ref path) = self.diff_file {
            Ok(InputMode::DiffFile(path.clone()))
        } else if self.diff_stdin {
            Ok(InputMode::Stdin)
        } else if let Some(ref base) = self.diff_base {
            Ok(InputMode::GitBase(base.clone()))
        } else if let Some(ref path) = self.scan {
            Ok(InputMode::DirectPath(path.clone()))
        } else {
            unreachable!()
        }
    }
}

// InputMode is defined in models/ and re-exported here for convenience.
pub use nitpik::models::InputMode;

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a ReviewArgs with specified inputs and defaults for the rest.
    fn make_args(
        diff_file: Option<&str>,
        diff_base: Option<&str>,
        scan: Option<&str>,
    ) -> ReviewArgs {
        make_args_full(diff_file, false, diff_base, scan)
    }

    fn make_args_full(
        diff_file: Option<&str>,
        diff_stdin: bool,
        diff_base: Option<&str>,
        scan: Option<&str>,
    ) -> ReviewArgs {
        ReviewArgs {
            path: PathBuf::from("."),
            diff_file: diff_file.map(PathBuf::from),
            diff_stdin,
            diff_base: diff_base.map(String::from),
            scan: scan.map(PathBuf::from),
            profile: vec!["backend".to_string()],
            profile_dir: None,
            tag: vec![],
            format: OutputFormat::Terminal,
            fail_on: None,
            no_fail: false,
            agent: false,
            max_turns: 10,
            max_tool_calls: 10,
            scan_secrets: false,
            secrets_rules: None,
            max_concurrent: 5,
            no_cache: false,
            no_prior_context: false,
            max_prior_findings: None,
            quiet: false,
            no_project_docs: false,
            no_commit_context: false,
            exclude_doc: vec![],
        }
    }

    #[test]
    fn validate_no_input() {
        let args = make_args(None, None, None);
        let result = args.validate_input();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("one input source is required"));
    }

    #[test]
    fn validate_multiple_inputs() {
        let args = make_args(Some("diff.patch"), Some("main"), None);
        let result = args.validate_input();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("only one input source allowed")
        );
    }

    #[test]
    fn validate_diff_file_input() {
        let args = make_args(Some("diff.patch"), None, None);
        let mode = args.validate_input().unwrap();
        assert!(matches!(mode, InputMode::DiffFile(_)));
    }

    #[test]
    fn validate_diff_base_input() {
        let args = make_args(None, Some("main"), None);
        let mode = args.validate_input().unwrap();
        assert!(matches!(mode, InputMode::GitBase(_)));
    }

    #[test]
    fn validate_scan_input() {
        let args = make_args(None, None, Some("src/"));
        let mode = args.validate_input().unwrap();
        assert!(matches!(mode, InputMode::DirectPath(_)));
    }

    #[test]
    fn validate_stdin_input() {
        let args = make_args_full(None, true, None, None);
        let mode = args.validate_input().unwrap();
        assert!(matches!(mode, InputMode::Stdin));
    }

    #[test]
    fn validate_stdin_conflicts_with_diff_file() {
        let args = make_args_full(Some("diff.patch"), true, None, None);
        let result = args.validate_input();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("only one input source allowed")
        );
    }

    #[test]
    fn validate_stdin_conflicts_with_diff_base() {
        let args = make_args_full(None, true, Some("main"), None);
        let result = args.validate_input();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("only one input source allowed")
        );
    }

    fn sample_finding() -> nitpik::models::finding::Finding {
        nitpik::models::finding::Finding {
            file: "src/main.rs".to_string(),
            line: 42,
            end_line: None,
            severity: nitpik::models::Severity::Warning,
            title: "Test issue".to_string(),
            message: "This is a test finding".to_string(),
            suggestion: Some("Fix it".to_string()),
            agent: "backend".to_string(),
        }
    }

    #[test]
    fn output_format_render_terminal() {
        let findings = vec![sample_finding()];
        let output = OutputFormat::Terminal.render(&findings);
        assert!(!output.is_empty());
        assert!(output.contains("src/main.rs"));
    }

    #[test]
    fn output_format_render_json() {
        let findings = vec![sample_finding()];
        let output = OutputFormat::Json.render(&findings);
        assert!(!output.is_empty());
        // JSON output should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_object() || parsed.is_array());
    }

    #[test]
    fn output_format_render_github() {
        let findings = vec![sample_finding()];
        let output = OutputFormat::Github.render(&findings);
        assert!(!output.is_empty());
        // GitHub Actions format uses ::warning:: annotations
        assert!(output.contains("::warning"));
    }

    #[test]
    fn output_format_render_bitbucket() {
        let findings = vec![sample_finding()];
        let output = OutputFormat::Bitbucket.render(&findings);
        assert!(!output.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("annotations").is_some());
    }

    #[test]
    fn output_format_render_forgejo() {
        let findings = vec![sample_finding()];
        let output = OutputFormat::Forgejo.render(&findings);
        assert!(!output.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["event"], "COMMENT");
        assert!(parsed.get("comments").is_some());
    }

    #[test]
    fn output_format_render_empty_findings() {
        // All formats should handle empty findings without panicking
        let empty: Vec<nitpik::models::finding::Finding> = vec![];
        let _ = OutputFormat::Terminal.render(&empty);
        let _ = OutputFormat::Json.render(&empty);
        let _ = OutputFormat::Github.render(&empty);
        let _ = OutputFormat::Bitbucket.render(&empty);
        let _ = OutputFormat::Forgejo.render(&empty);
    }

    #[test]
    fn quiet_flag_defaults_to_false() {
        let args = make_args(None, Some("main"), None);
        assert!(!args.quiet);
    }

    #[test]
    fn quiet_flag_can_be_set() {
        let mut args = make_args(None, Some("main"), None);
        args.quiet = true;
        assert!(args.quiet);
    }

    #[test]
    fn quiet_flag_parsed_long() {
        let cli =
            Cli::try_parse_from(["nitpik", "review", "--diff-base", "main", "--quiet"]).unwrap();
        match cli.command {
            Command::Review(args) => assert!(args.quiet),
            _ => panic!("expected Review command"),
        }
    }

    #[test]
    fn quiet_flag_parsed_short() {
        let cli = Cli::try_parse_from(["nitpik", "review", "--diff-base", "main", "-q"]).unwrap();
        match cli.command {
            Command::Review(args) => assert!(args.quiet),
            _ => panic!("expected Review command"),
        }
    }

    #[test]
    fn quiet_flag_absent_by_default() {
        let cli = Cli::try_parse_from(["nitpik", "review", "--diff-base", "main"]).unwrap();
        match cli.command {
            Command::Review(args) => assert!(!args.quiet),
            _ => panic!("expected Review command"),
        }
    }
}
