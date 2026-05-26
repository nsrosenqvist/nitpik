import * as vscode from "vscode";
import { randomBytes } from "node:crypto";
import { getConfig } from "./config";
import { log, logError } from "./log";

const SECRET_KEY = "nitpik.licenseKey";

/**
 * Manages the nitpik license key (`nkp_live_…`).
 *
 * Primary flow: a browser deep-link to `nitpik.dev/auth/ide/authorize`, which
 * redirects back via `vscode://nsrosenqvist.nitpik/auth?code=…&state=…`; the
 * code is exchanged for a key. Fallback: manual paste (for remote/headless).
 * The key is stored in the OS keychain via `SecretStorage`.
 */
export class AuthService implements vscode.UriHandler {
  private readonly _onDidChange = new vscode.EventEmitter<void>();
  /** Fires whenever the signed-in state changes (login or logout). */
  readonly onDidChange = this._onDidChange.event;

  /** State token for the in-flight deep-link authorization, if any. */
  private pendingState: string | undefined;

  constructor(private readonly secrets: vscode.SecretStorage) {}

  async getKey(): Promise<string | undefined> {
    return this.secrets.get(SECRET_KEY);
  }

  async isSignedIn(): Promise<boolean> {
    return !!(await this.getKey());
  }

  async setKey(key: string): Promise<void> {
    await this.secrets.store(SECRET_KEY, key.trim());
    this._onDidChange.fire();
  }

  async signOut(): Promise<void> {
    await this.secrets.delete(SECRET_KEY);
    this._onDidChange.fire();
    vscode.window.showInformationMessage("nitpik: signed out.");
  }

  /** Run the interactive sign-in flow (browser deep-link or manual paste). */
  async signIn(): Promise<void> {
    const choice = await vscode.window.showQuickPick(
      [
        { label: "$(globe) Sign in with browser", id: "browser", description: "Recommended" },
        { label: "$(key) Paste an API key", id: "paste", description: "For remote/headless editors" },
      ],
      { placeHolder: "How would you like to sign in to nitpik?" },
    );
    if (!choice) {
      return;
    }
    if (choice.id === "browser") {
      await this.signInWithBrowser();
    } else {
      await this.signInWithPaste();
    }
  }

  private async signInWithBrowser(): Promise<void> {
    const state = randomBytes(16).toString("hex");
    this.pendingState = state;
    const base = getConfig().serverUrl;
    const url = `${base}/auth/ide/authorize?editor=vscode&state=${encodeURIComponent(state)}`;
    log(`auth: opening browser ${url}`);
    await vscode.env.openExternal(vscode.Uri.parse(url));
  }

  private async signInWithPaste(): Promise<void> {
    const key = await vscode.window.showInputBox({
      prompt: "Paste your nitpik API key (nkp_live_…)",
      password: true,
      ignoreFocusOut: true,
      validateInput: (v) => (/^nkp_(live|test)_/.test(v.trim()) ? undefined : "Expected a key starting with nkp_live_"),
    });
    if (key) {
      await this.setKey(key);
      vscode.window.showInformationMessage("nitpik: signed in.");
    }
  }

  /** Handle the `vscode://…/auth?code=…&state=…` deep-link callback. */
  async handleUri(uri: vscode.Uri): Promise<void> {
    const params = new URLSearchParams(uri.query);
    const code = params.get("code");
    const state = params.get("state");

    if (!code) {
      vscode.window.showErrorMessage("nitpik: sign-in failed — no code received.");
      return;
    }
    if (!this.pendingState || state !== this.pendingState) {
      vscode.window.showErrorMessage("nitpik: sign-in failed — state mismatch (possible CSRF).");
      return;
    }
    this.pendingState = undefined;

    try {
      const key = await this.exchangeCode(code, state);
      await this.setKey(key);
      vscode.window.showInformationMessage("nitpik: signed in successfully.");
    } catch (err) {
      logError("auth: code exchange failed", err);
      vscode.window.showErrorMessage(`nitpik: sign-in failed — ${err instanceof Error ? err.message : String(err)}`);
    }
  }

  /** Exchange a one-time authorization code for an API key. */
  private async exchangeCode(code: string, state: string): Promise<string> {
    const base = getConfig().serverUrl;
    const res = await fetch(`${base}/v1/ide/exchange-code`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify({ code, state, editor: "vscode" }),
    });
    if (!res.ok) {
      throw new Error(`exchange failed (HTTP ${res.status})`);
    }
    const body = (await res.json()) as { key?: string };
    if (!body.key) {
      throw new Error("no key in exchange response");
    }
    return body.key;
  }

  dispose(): void {
    this._onDidChange.dispose();
  }
}
