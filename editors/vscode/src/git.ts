import * as vscode from "vscode";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

/** Absolute path of the first workspace folder, or undefined. */
export function workspaceRoot(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

async function git(args: string[], cwd: string): Promise<string> {
  const { stdout } = await execFileAsync("git", args, { cwd, timeout: 5_000 });
  return stdout.trim();
}

/**
 * Resolve the ref to diff against for "Review Changes".
 *
 * Uses the configured `nitpik.diffBase` when set, otherwise the merge-base
 * between HEAD and the repo's default branch (origin/HEAD), falling back to
 * "HEAD" (uncommitted changes) when none can be determined.
 */
export async function resolveDiffBase(configured: string, cwd: string): Promise<string> {
  if (configured.trim()) {
    return configured.trim();
  }
  try {
    // e.g. "origin/main"
    const defaultRef = await git(["rev-parse", "--abbrev-ref", "origin/HEAD"], cwd);
    const mergeBase = await git(["merge-base", "HEAD", defaultRef], cwd);
    return mergeBase || "HEAD";
  } catch {
    return "HEAD";
  }
}
