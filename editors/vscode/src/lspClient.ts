import * as vscode from "vscode";
import {
  LanguageClient,
  type LanguageClientOptions,
  type ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";
import type { AuthService } from "./auth";
import type { ShimServer } from "./shim/server";
import { buildReviewEnv } from "./spawnEnv";
import { workspaceRoot, resolveDiffBase } from "./git";
import { getConfig } from "./config";
import { log, logError } from "./log";

const COMMAND = {
  reviewChanges: "nitpik.reviewChanges",
  reviewFile: "nitpik.reviewFile",
  reviewWorkspace: "nitpik.reviewWorkspace",
  reviewFresh: "nitpik.reviewFresh",
} as const;

/**
 * Manages the `nitpik serve lsp` LanguageClient: diagnostics + code actions
 * are produced server-side. Editor commands are thin wrappers that forward to
 * the server via `workspace/executeCommand`.
 */
export class NitpikLspClient {
  private client: LanguageClient | undefined;

  constructor(
    private readonly binaryPath: string,
    private readonly auth: AuthService,
    private readonly shim: ShimServer,
    private readonly output: vscode.OutputChannel,
  ) {}

  async start(): Promise<void> {
    if (this.client) {
      return;
    }
    const cwd = workspaceRoot();
    const env = { ...process.env, ...(await buildReviewEnv(this.auth, this.shim)) };

    const serverOptions: ServerOptions = {
      command: this.binaryPath,
      args: ["serve", "lsp", ...(cwd ? ["--path", cwd] : [])],
      transport: TransportKind.stdio,
      options: { env, cwd },
    };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [{ scheme: "file" }],
      outputChannel: this.output,
    };

    this.client = new LanguageClient("nitpik", "nitpik", serverOptions, clientOptions);
    await this.client.start();
    log("lsp: client started");
  }

  async stop(): Promise<void> {
    await this.client?.stop();
    this.client = undefined;
  }

  /** Restart to pick up new env (auth/config change). */
  async restart(): Promise<void> {
    await this.stop();
    await this.start();
  }

  private async exec(command: string, args: unknown[]): Promise<void> {
    if (!this.client) {
      vscode.window.showWarningMessage("nitpik: language server is not running.");
      return;
    }
    try {
      await this.client.sendRequest("workspace/executeCommand", { command, arguments: args });
    } catch (err) {
      logError(`lsp: command ${command} failed`, err);
      vscode.window.showErrorMessage(`nitpik: ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  async reviewChanges(fresh = false): Promise<void> {
    const cwd = workspaceRoot();
    const base = cwd ? await resolveDiffBase(getConfig().diffBase, cwd) : "HEAD";
    await this.exec(fresh ? COMMAND.reviewFresh : COMMAND.reviewChanges, [base]);
  }

  async reviewFile(): Promise<void> {
    const uri = vscode.window.activeTextEditor?.document.uri;
    if (!uri) {
      vscode.window.showWarningMessage("nitpik: no active file to review.");
      return;
    }
    await this.exec(COMMAND.reviewFile, [uri.toString()]);
  }

  async reviewWorkspace(): Promise<void> {
    await this.exec(COMMAND.reviewWorkspace, []);
  }
}
