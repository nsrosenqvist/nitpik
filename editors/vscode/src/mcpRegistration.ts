import * as vscode from "vscode";
import type { AuthService } from "./auth";
import type { ShimServer } from "./shim/server";
import { buildReviewEnv } from "./spawnEnv";
import { workspaceRoot } from "./git";
import { log, logError } from "./log";

export interface McpDeps {
  auth: AuthService;
  shim: ShimServer;
  binaryPath: string;
}

/**
 * Register an MCP server definition provider so Copilot agents can invoke
 * nitpik. Unlike a remote HTTP MCP server, this spawns the local
 * `nitpik serve mcp` binary over stdio, passing the shim env so agent-driven
 * reviews also use the editor's model.
 */
export function registerMcpServer(deps: McpDeps): vscode.Disposable {
  if (!vscode.lm?.registerMcpServerDefinitionProvider) {
    logError("mcp: registerMcpServerDefinitionProvider unavailable (VS Code < 1.95?)");
    return new vscode.Disposable(() => {});
  }

  const changeEmitter = new vscode.EventEmitter<void>();
  const disposables: vscode.Disposable[] = [changeEmitter];

  const provider: vscode.McpServerDefinitionProvider = {
    onDidChangeMcpServerDefinitions: changeEmitter.event,
    async provideMcpServerDefinitions() {
      if (!(await deps.auth.isSignedIn())) {
        log("mcp: not signed in — no server definition");
        return [];
      }
      const cwd = workspaceRoot();
      const env = await buildReviewEnv(deps.auth, deps.shim);
      const def = new vscode.McpStdioServerDefinition(
        "nitpik",
        deps.binaryPath,
        ["serve", "mcp", ...(cwd ? ["--path", cwd] : [])],
        env,
      );
      if (cwd) {
        def.cwd = vscode.Uri.file(cwd);
      }
      log("mcp: providing stdio server definition");
      return [def];
    },
  };

  disposables.push(vscode.lm.registerMcpServerDefinitionProvider("nitpik", provider));

  // Re-signal when auth or relevant config changes so VS Code re-fetches.
  disposables.push(deps.auth.onDidChange(() => changeEmitter.fire()));
  disposables.push(
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("nitpik")) {
        changeEmitter.fire();
      }
    }),
  );

  return vscode.Disposable.from(...disposables);
}
