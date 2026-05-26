import * as vscode from "vscode";
import * as fs from "node:fs";
import * as path from "node:path";
import { getConfig } from "./config";

/**
 * Resolve the nitpik binary to spawn.
 *
 * Order: the `nitpik.path` setting → the binary bundled in the (platform-
 * specific) VSIX under `bin/` → the bare name `nitpik` (found on `PATH`).
 */
export function resolveBinary(context: vscode.ExtensionContext): string {
  const configured = getConfig().binaryPath.trim();
  if (configured) {
    return configured;
  }

  const exe = process.platform === "win32" ? "nitpik.exe" : "nitpik";
  const bundled = path.join(context.extensionPath, "bin", exe);
  if (fs.existsSync(bundled)) {
    return bundled;
  }

  // Fall back to PATH lookup by the host when spawned.
  return "nitpik";
}
