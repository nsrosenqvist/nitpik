# nitpik for VS Code

AI-powered code review in your editor and Copilot agents, powered by the
[nitpik](https://nitpik.dev) engine.

## What it does

- **Reviews as diagnostics.** Runs `nitpik serve lsp` and surfaces findings in
  the Problems panel. Reviews fire on demand — `nitpik: Review Changes`,
  `Review Current File`, `Review Workspace` — never on every keystroke.
  `nitpik: Re-review from Scratch` ignores the cache and prior findings.
- **Uses your Copilot model.** By default (`nitpik.modelSource: "copilot"`) the
  extension hosts a localhost OpenAI-compatible endpoint backed by
  `vscode.lm`, so reviews run on the editor's own model — no separate API key.
  Set `nitpik.modelSource: "byom"` to use the provider in your `.nitpik.toml`.
- **Available to Copilot agents.** Auto-registers `nitpik serve mcp` as an MCP
  server, so Copilot's agent can invoke nitpik review tools directly.

## Setup

1. Install the [nitpik binary](https://nitpik.dev) (bundled with the
   marketplace build; otherwise set `nitpik.path` or put it on `PATH`).
2. Run **nitpik: Sign In** and authenticate (browser flow, or paste an
   `nkp_live_…` key for remote/headless editors). An active subscription is
   required for editor and agent reviews.
3. Open a repo with changes and run **nitpik: Review Changes**.

## Settings

| Setting | Default | Description |
| --- | --- | --- |
| `nitpik.modelSource` | `copilot` | `copilot` (editor model via bridge) or `byom` (`.nitpik.toml` provider). |
| `nitpik.copilotModel` | `""` | Preferred Copilot model family (empty = default). |
| `nitpik.diffBase` | `""` | Git ref for "Review Changes" (empty = merge-base with default branch). |
| `nitpik.reviewOnSave` | `false` | Auto-review a file on save (debounced). |
| `nitpik.path` | `""` | Path to the nitpik binary (empty = bundled, then `PATH`). |
| `nitpik.serverUrl` | `https://nitpik.dev` | nitpik service base URL. |
