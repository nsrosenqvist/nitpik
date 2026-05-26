import * as vscode from "vscode";

export interface NitpikConfig {
  /** Base URL of the nitpik service (no trailing slash). */
  serverUrl: string;
  /** Explicit binary path, or "" to use bundled/PATH resolution. */
  binaryPath: string;
  /** Which model runs editor-launched reviews. */
  modelSource: "copilot" | "byom";
  /** Preferred Copilot model family, or "" for the default. */
  copilotModel: string;
  /** Git ref to diff against, or "" for the merge-base with the default branch. */
  diffBase: string;
  /** Auto-review on save (debounced). Off by default. */
  reviewOnSave: boolean;
}

export function getConfig(): NitpikConfig {
  const c = vscode.workspace.getConfiguration("nitpik");
  return {
    serverUrl: c.get<string>("serverUrl", "https://nitpik.dev").replace(/\/+$/, ""),
    binaryPath: c.get<string>("path", ""),
    modelSource: c.get<"copilot" | "byom">("modelSource", "copilot"),
    copilotModel: c.get<string>("copilotModel", ""),
    diffBase: c.get<string>("diffBase", ""),
    reviewOnSave: c.get<boolean>("reviewOnSave", false),
  };
}
