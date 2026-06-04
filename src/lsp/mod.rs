//! LSP server (`nitpik serve lsp`).
//!
//! Surfaces nitpik findings as native diagnostics in any LSP client (VS Code,
//! Neovim, Helix, JetBrains, …). Reviews are **never** triggered by document
//! sync events — only by explicit commands / code actions — because each review
//! is an LLM call. Document sync is tracked only so the server knows what's
//! open. Reviews run through [`crate::review::execute_review`], so findings
//! match the CLI exactly. The model comes from the process's provider config
//! (shim → Copilot in VS Code, BYOM elsewhere).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{Mutex, RwLock};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server, jsonrpc};

use crate::config::Config;
use crate::diff;
use crate::env::Env;
use crate::models::InputMode;
use crate::models::finding::Finding;
use crate::review::{self, NoopProgress, ReviewOptions};

/// Explicit review commands the server advertises via `executeCommand`.
mod command {
    /// Review changes vs a git base (arg 0: optional base ref).
    pub const REVIEW_CHANGES: &str = "nitpik.reviewChanges";
    /// Review a single file (arg 0: file URI string).
    pub const REVIEW_FILE: &str = "nitpik.reviewFile";
    /// Review the whole workspace directly.
    pub const REVIEW_WORKSPACE: &str = "nitpik.reviewWorkspace";
    /// Re-review changes ignoring cache + prior findings (arg 0: optional base).
    pub const REVIEW_FRESH: &str = "nitpik.reviewFresh";

    pub const ALL: &[&str] = &[REVIEW_CHANGES, REVIEW_FILE, REVIEW_WORKSPACE, REVIEW_FRESH];
}

struct Backend {
    client: Client,
    /// Workspace root (from `initialize`, falling back to the launch `--path`).
    root: RwLock<PathBuf>,
    /// URIs we last published diagnostics for, so we can clear stale ones.
    published: Mutex<HashSet<Url>>,
    /// Single-flight guard: only one review runs at a time.
    reviewing: Mutex<()>,
}

impl Backend {
    fn new(client: Client, root: PathBuf) -> Self {
        Self {
            client,
            root: RwLock::new(root),
            published: Mutex::new(HashSet::new()),
            reviewing: Mutex::new(()),
        }
    }

    /// Run a review (single-flight) and publish the resulting diagnostics.
    async fn review(&self, mode: InputMode, fresh: bool) {
        // Coalesce concurrent requests — reviews are expensive LLM calls.
        let _guard = match self.reviewing.try_lock() {
            Ok(g) => g,
            Err(_) => {
                self.client
                    .show_message(MessageType::INFO, "nitpik: a review is already running")
                    .await;
                return;
            }
        };

        let root = self.root.read().await.clone();
        self.client
            .log_message(MessageType::INFO, "nitpik: running review…")
            .await;

        match self.run_review(&root, mode, fresh).await {
            Ok((repo_root, findings)) => self.publish(&repo_root, findings).await,
            Err(e) => {
                self.client
                    .show_message(MessageType::ERROR, format!("nitpik review failed: {e:#}"))
                    .await;
            }
        }
    }

    /// Execute a review and return `(repo_root, findings)`.
    async fn run_review(
        &self,
        root: &Path,
        input_mode: InputMode,
        fresh: bool,
    ) -> Result<(PathBuf, Vec<Finding>)> {
        let repo_root = review::resolve_repo_root(root).await?;
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
        if diffs.is_empty() {
            return Ok((PathBuf::from(&repo_root), Vec::new()));
        }

        let is_path_scan = matches!(input_mode, InputMode::DirectPath(_));
        let mut options = server_options(&config);
        if fresh {
            options.no_cache = true;
            options.no_prior_context = true;
        }

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
            // Editor reviews have no PR context.
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

        Ok((PathBuf::from(&repo_root), output.findings))
    }

    /// Publish findings as diagnostics grouped by file, clearing any files that
    /// previously had findings but no longer do.
    async fn publish(&self, repo_root: &Path, findings: Vec<Finding>) {
        let mut by_uri: std::collections::HashMap<Url, Vec<Diagnostic>> =
            std::collections::HashMap::new();

        for f in &findings {
            let abs = repo_root.join(&f.file);
            let Ok(uri) = Url::from_file_path(&abs) else {
                continue;
            };
            by_uri
                .entry(uri)
                .or_default()
                .push(finding_to_diagnostic(f));
        }

        let new_uris: HashSet<Url> = by_uri.keys().cloned().collect();

        // Clear files that had findings before but don't now.
        let mut published = self.published.lock().await;
        for stale in published.difference(&new_uris) {
            self.client
                .publish_diagnostics(stale.clone(), Vec::new(), None)
                .await;
        }

        for (uri, diags) in by_uri {
            self.client.publish_diagnostics(uri, diags, None).await;
        }
        *published = new_uris;

        let n = findings.len();
        self.client
            .log_message(
                MessageType::INFO,
                format!("nitpik: review complete — {n} finding(s)"),
            )
            .await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> jsonrpc::Result<InitializeResult> {
        // Prefer a workspace folder, then the (deprecated) root_uri.
        let folder = params
            .workspace_folders
            .as_ref()
            .and_then(|f| f.first())
            .map(|f| f.uri.clone())
            .or(params.root_uri);
        if let Some(uri) = folder
            && let Ok(path) = uri.to_file_path()
        {
            *self.root.write().await = path;
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "nitpik".into(),
                version: Some(crate::constants::VERSION.into()),
            }),
            capabilities: ServerCapabilities {
                // Tracked for context only — never triggers a review.
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: command::ALL.iter().map(|s| s.to_string()).collect(),
                    work_done_progress_options: Default::default(),
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(
                MessageType::INFO,
                "nitpik LSP ready — reviews run on demand",
            )
            .await;
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }

    /// Offer an explicit "review this file" action. Reviews never run from
    /// document-sync handlers, so this is the in-editor entry point.
    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> jsonrpc::Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let action = CodeAction {
            title: "nitpik: Review this file".to_string(),
            kind: Some(CodeActionKind::EMPTY),
            command: Some(Command {
                title: "Review this file".to_string(),
                command: command::REVIEW_FILE.to_string(),
                arguments: Some(vec![serde_json::Value::String(uri.to_string())]),
            }),
            ..Default::default()
        };
        Ok(Some(vec![CodeActionOrCommand::CodeAction(action)]))
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> jsonrpc::Result<Option<serde_json::Value>> {
        let first_string = |args: &[serde_json::Value]| -> Option<String> {
            args.first().and_then(|v| v.as_str().map(str::to_string))
        };

        match params.command.as_str() {
            command::REVIEW_CHANGES => {
                let base = first_string(&params.arguments).unwrap_or_else(|| "HEAD".to_string());
                self.review(InputMode::GitBase(base), false).await;
            }
            command::REVIEW_FRESH => {
                let base = first_string(&params.arguments).unwrap_or_else(|| "HEAD".to_string());
                self.review(InputMode::GitBase(base), true).await;
            }
            command::REVIEW_WORKSPACE => {
                let root = self.root.read().await.clone();
                self.review(InputMode::DirectPath(root), false).await;
            }
            command::REVIEW_FILE => {
                let path = match first_string(&params.arguments)
                    .and_then(|s| Url::parse(&s).ok())
                    .and_then(|u| u.to_file_path().ok())
                {
                    Some(p) => p,
                    None => {
                        self.client
                            .show_message(
                                MessageType::ERROR,
                                "nitpik: reviewFile requires a file URI argument",
                            )
                            .await;
                        return Ok(None);
                    }
                };
                self.review(InputMode::DirectPath(path), false).await;
            }
            other => {
                self.client
                    .show_message(
                        MessageType::ERROR,
                        format!("nitpik: unknown command {other}"),
                    )
                    .await;
            }
        }
        Ok(None)
    }
}

/// Map a nitpik [`Finding`] to an LSP [`Diagnostic`] spanning the finding's
/// line range.
fn finding_to_diagnostic(f: &Finding) -> Diagnostic {
    let start_line = f.line.saturating_sub(1);
    let end_line = f.end_line.unwrap_or(f.line).saturating_sub(1);
    let mut message = format!("{}\n{}", f.title, f.message);
    if let Some(suggestion) = &f.suggestion {
        message.push_str(&format!("\n\nSuggestion: {suggestion}"));
    }
    Diagnostic {
        range: Range {
            start: Position {
                line: start_line,
                character: 0,
            },
            end: Position {
                line: end_line,
                character: u32::MAX,
            },
        },
        severity: Some(severity_to_lsp(f.severity)),
        source: Some("nitpik".to_string()),
        code: Some(NumberOrString::String(f.agent.clone())),
        message,
        ..Default::default()
    }
}

fn severity_to_lsp(s: crate::models::Severity) -> DiagnosticSeverity {
    use crate::models::Severity;
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
    }
}

/// Default review options for the LSP server (agentic mode — same rationale as
/// the MCP server).
fn server_options(config: &Config) -> ReviewOptions {
    ReviewOptions {
        agent_policy: crate::models::AgentPolicy::On,
        scan_secrets: config.secrets.enabled,
        scan_threats: config.threats.enabled,
        ..ReviewOptions::default()
    }
}

/// Serve the LSP server over stdio until the client disconnects.
pub async fn serve(root: PathBuf) -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(|client| Backend::new(client, root));
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}
