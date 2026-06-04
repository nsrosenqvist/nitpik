//! MCP server (`nitpik serve mcp`).
//!
//! Exposes nitpik's review engine as Model Context Protocol tools over stdio
//! so agents (GitHub Copilot, Claude Code, Cursor, …) can invoke reviews. The
//! tools wrap [`crate::review::execute_review`], so results are identical to
//! the CLI. The LLM that performs the review comes from the process's provider
//! configuration: the VS Code extension spawns this with env pointing at its
//! localhost shim (Copilot's model); standalone / Claude Code uses BYOM.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    ErrorData, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};

use crate::config::Config;
use crate::diff;
use crate::env::Env;
use crate::models::InputMode;
use crate::output::OutputFormatter;
use crate::output::json::JsonFormatter;
use crate::review::{self, NoopProgress, ReviewOptions};

/// nitpik MCP server. Holds the workspace root the tools operate within.
#[derive(Clone)]
pub struct NitpikMcpServer {
    root: PathBuf,
    tool_router: ToolRouter<Self>,
}

/// Arguments for `nitpik_review_diff`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ReviewDiffArgs {
    /// Git ref to diff against. Defaults to `HEAD` (review uncommitted changes).
    #[serde(default)]
    base: Option<String>,
}

/// Arguments for `nitpik_review_files`.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ReviewFilesArgs {
    /// File or directory to review directly, relative to the workspace root.
    path: String,
}

#[tool_router(router = tool_router)]
impl NitpikMcpServer {
    /// Build a server rooted at `root` (typically the editor workspace dir).
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "nitpik_review_diff",
        description = "Review the repository's changes against a git ref (default HEAD) and return findings as JSON."
    )]
    async fn review_diff(
        &self,
        Parameters(args): Parameters<ReviewDiffArgs>,
    ) -> Result<String, ErrorData> {
        let base = args.base.unwrap_or_else(|| "HEAD".to_string());
        self.run(InputMode::GitBase(base)).await
    }

    #[tool(
        name = "nitpik_review_files",
        description = "Review a file or directory directly (no git required) and return findings as JSON."
    )]
    async fn review_files(
        &self,
        Parameters(args): Parameters<ReviewFilesArgs>,
    ) -> Result<String, ErrorData> {
        let path = self.root.join(&args.path);
        self.run(InputMode::DirectPath(path)).await
    }

    #[tool(
        name = "nitpik_list_profiles",
        description = "List the available review agent profiles (name, description, tags)."
    )]
    async fn list_profiles(&self) -> Result<String, ErrorData> {
        let agents = crate::agents::list_all_profiles(None)
            .await
            .map_err(|e| ErrorData::internal_error(format!("{e:#}"), None))?;
        let profiles: Vec<_> = agents
            .iter()
            .map(|a| {
                serde_json::json!({
                    "name": a.profile.name,
                    "description": a.profile.description,
                    "tags": a.profile.tags,
                })
            })
            .collect();
        Ok(
            serde_json::to_string_pretty(&serde_json::json!({ "profiles": profiles }))
                .unwrap_or_else(|_| "{}".to_string()),
        )
    }
}

impl NitpikMcpServer {
    /// Run a review for `input_mode` and return findings as a JSON string,
    /// mapping engine errors to MCP errors.
    async fn run(&self, input_mode: InputMode) -> Result<String, ErrorData> {
        self.run_inner(input_mode)
            .await
            .map_err(|e| ErrorData::internal_error(format!("{e:#}"), None))
    }

    async fn run_inner(&self, input_mode: InputMode) -> Result<String> {
        let repo_root = review::resolve_repo_root(&self.root).await?;
        let repo_root_path = Path::new(&repo_root);
        let env = Env::real();
        let config = Config::load(Some(repo_root_path), &env)?;

        // Editor/agent reviews require an active subscription.
        review::require_entitlement(&config, &env)
            .await
            .map_err(|m| anyhow::anyhow!(m))?;

        let diff_source = diff::get_diff_source(&input_mode, repo_root_path).await?;
        let parsed_diffs;
        let diffs: &[crate::models::FileDiff<'_>] = match &diff_source {
            diff::DiffSource::Raw(content) => {
                parsed_diffs = diff::parser::parse_unified_diff(content);
                &parsed_diffs
            }
            diff::DiffSource::Scanned(d) => d,
        };

        let formatter = JsonFormatter;
        if diffs.is_empty() {
            return Ok(formatter.format(&[]));
        }

        let is_path_scan = matches!(input_mode, InputMode::DirectPath(_));
        let options = server_options(&config);

        // One provider, injected into both profile resolution and the review.
        let provider: Arc<dyn crate::providers::ReviewProvider> = Arc::new(
            crate::providers::rig::RigProvider::new(
                config.provider.clone(),
                repo_root_path.to_path_buf(),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?,
        );

        let agent_defs = review::resolve_agents(
            Some(provider.as_ref()),
            &options,
            &config,
            diffs,
            repo_root_path,
        )
        .await?
        .agents;
        let commit_log =
            review::build_commit_log(options.no_commit_context, &input_mode, repo_root_path).await;
        let baseline = crate::context::build_baseline_context(
            repo_root_path,
            diffs,
            &config,
            options.no_project_docs,
            &options.exclude_doc,
            commit_log,
            // Editor/agent reviews have no PR context.
            None,
        )
        .await;

        let output = review::execute_review(
            provider,
            &config,
            &repo_root,
            diffs,
            is_path_scan,
            &agent_defs,
            baseline,
            &options,
            Arc::new(NoopProgress),
        )
        .await?;

        Ok(formatter.format(&output.findings))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for NitpikMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(
                "nitpik — AI-powered code review. Tools review code changes and return findings as JSON.",
            )
    }
}

/// Default review options for headless server runs.
///
/// Reviews run in **agentic mode** so findings come back via the
/// `submit_findings` function-call tool — the model bridge (`vscode.lm` via the
/// extension's shim) does not expose OpenAI `response_format`.
fn server_options(config: &Config) -> ReviewOptions {
    ReviewOptions {
        use_agent: true,
        scan_secrets: config.secrets.enabled,
        scan_threats: config.threats.enabled,
        ..ReviewOptions::default()
    }
}

/// Serve the MCP server over stdio until the client disconnects.
pub async fn serve(root: PathBuf) -> Result<()> {
    let server = NitpikMcpServer::new(root);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
