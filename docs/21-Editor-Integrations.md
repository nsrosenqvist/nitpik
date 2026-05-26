# Editor & Agent Integrations

nitpik runs as a long-lived server so editors and AI agents can use the same
review engine as the CLI. Two transports are exposed under `nitpik serve`:

- **`nitpik serve lsp`** — a Language Server that publishes findings as
  diagnostics in any LSP client (VS Code, Neovim, Helix, JetBrains, …).
- **`nitpik serve mcp`** — a Model Context Protocol server exposing nitpik as
  tools for agents (GitHub Copilot, Claude Code, Cursor, …).

> Editor and agent reviews require an active subscription. Reviews run in
> agentic mode and use whatever provider the process is configured with.

## VS Code

Install the **nitpik** extension from the Marketplace. It:

- runs `nitpik serve lsp` and shows findings in the Problems panel;
- by default runs reviews on your **Copilot model** via a local bridge
  (`nitpik.modelSource: "copilot"`) — no extra API key — or uses your
  `.nitpik.toml` provider with `nitpik.modelSource: "byom"`;
- auto-registers `nitpik serve mcp` so Copilot's agent can call nitpik too.

Commands: **Review Changes**, **Review Current File**, **Review Workspace**,
**Re-review from Scratch** (ignores cache + prior findings). See the
[extension README](https://github.com/nsrosenqvist/nitpik/tree/main/editors/vscode).

## Claude Code (and other MCP agents)

nitpik ships a Claude Code plugin (`editors/claude-code`) that registers the
MCP server. Without the plugin:

```bash
claude mcp add nitpik -- nitpik serve mcp
```

Claude Code does not expose its model to MCP servers, so reviews use your
configured provider (BYOM) — set an API key and `.nitpik.toml`. The same MCP
server works for Cursor, Codex, Gemini CLI, and other agents.

Tools: `nitpik_review_diff`, `nitpik_review_files`, `nitpik_list_profiles`.

## Neovim / Helix / JetBrains (LSP)

Point your editor's LSP client at `nitpik serve lsp` (stdio). Reviews are
triggered explicitly via the server commands (`nitpik.reviewChanges`,
`nitpik.reviewFile`, `nitpik.reviewWorkspace`, `nitpik.reviewFresh`) or the
"Review this file" code action — never on every save (each review is an LLM
call). These editors use your `.nitpik.toml` provider (BYOM).

### Neovim example

```lua
vim.lsp.start({
  name = "nitpik",
  cmd = { "nitpik", "serve", "lsp", "--path", vim.fn.getcwd() },
  root_dir = vim.fs.dirname(vim.fs.find({ ".git" }, { upward = true })[1]),
})

-- Trigger a review of the changes on the current branch:
vim.keymap.set("n", "<leader>nr", function()
  vim.lsp.buf.execute_command({ command = "nitpik.reviewChanges", arguments = { "HEAD" } })
end)
```
