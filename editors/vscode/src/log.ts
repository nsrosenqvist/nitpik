import * as vscode from "vscode";

let channel: vscode.OutputChannel | undefined;

export function initLog(): vscode.OutputChannel {
  channel ??= vscode.window.createOutputChannel("nitpik");
  return channel;
}

export function log(message: string): void {
  channel?.appendLine(`[${new Date().toISOString()}] ${message}`);
}

export function logError(message: string, err?: unknown): void {
  const detail =
    err instanceof Error ? `${err.message}\n${err.stack ?? ""}` : err !== undefined ? String(err) : "";
  channel?.appendLine(`[${new Date().toISOString()}] ERROR: ${message}${detail ? `\n${detail}` : ""}`);
}
