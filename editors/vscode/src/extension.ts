import * as vscode from "vscode";
import { initLog, log } from "./log";
import { AuthService } from "./auth";
import { ShimServer } from "./shim/server";
import { NitpikLspClient } from "./lspClient";
import { registerMcpServer } from "./mcpRegistration";
import { resolveBinary } from "./binary";
import { getConfig } from "./config";

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = initLog();
  log("nitpik extension activating");

  const auth = new AuthService(context.secrets);
  context.subscriptions.push(auth, vscode.window.registerUriHandler(auth));

  // Localhost OpenAI shim backed by the editor's Copilot model. The model
  // family is read lazily so config changes take effect without a restart.
  const shim = new ShimServer(() => getConfig().copilotModel);
  try {
    await shim.start();
  } catch (err) {
    log(`shim: failed to start: ${err instanceof Error ? err.message : String(err)}`);
  }
  context.subscriptions.push({ dispose: () => shim.dispose() });

  const binaryPath = resolveBinary(context);

  const lsp = new NitpikLspClient(binaryPath, auth, shim, output);
  try {
    await lsp.start();
  } catch (err) {
    log(`lsp: failed to start: ${err instanceof Error ? err.message : String(err)}`);
  }
  context.subscriptions.push({ dispose: () => void lsp.stop() });

  context.subscriptions.push(registerMcpServer({ auth, shim, binaryPath }));

  // Restart the LSP server when auth or relevant config changes so the spawned
  // process picks up fresh env (license key / model source).
  context.subscriptions.push(auth.onDidChange(() => void lsp.restart()));
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (
        e.affectsConfiguration("nitpik.modelSource") ||
        e.affectsConfiguration("nitpik.copilotModel") ||
        e.affectsConfiguration("nitpik.path")
      ) {
        void lsp.restart();
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("nitpik.signIn", () => auth.signIn()),
    vscode.commands.registerCommand("nitpik.signOut", () => auth.signOut()),
    vscode.commands.registerCommand("nitpik.reviewChanges", () => lsp.reviewChanges(false)),
    vscode.commands.registerCommand("nitpik.reviewFresh", () => lsp.reviewChanges(true)),
    vscode.commands.registerCommand("nitpik.reviewFile", () => lsp.reviewFile()),
    vscode.commands.registerCommand("nitpik.reviewWorkspace", () => lsp.reviewWorkspace()),
  );

  // Opt-in review-on-save (off by default), debounced. Server-side single-flight
  // coalesces overlapping runs.
  const saveTimers = new Map<string, NodeJS.Timeout>();
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument((doc) => {
      if (!getConfig().reviewOnSave || doc.uri.scheme !== "file") {
        return;
      }
      const key = doc.uri.toString();
      clearTimeout(saveTimers.get(key));
      saveTimers.set(
        key,
        setTimeout(() => {
          saveTimers.delete(key);
          if (vscode.window.activeTextEditor?.document.uri.toString() === key) {
            void lsp.reviewFile();
          }
        }, 1_000),
      );
    }),
  );

  log("nitpik extension activated");
}

export function deactivate(): void {
  // Subscriptions (shim, LSP client) are disposed by VS Code.
}
