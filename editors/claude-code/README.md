# nitpik plugin for Claude Code

Exposes nitpik's review engine to Claude Code as MCP tools:

- `nitpik_review_diff` — review changes against a git ref (default `HEAD`)
- `nitpik_review_files` — review a file or directory directly
- `nitpik_list_profiles` — list available reviewer profiles

## Requirements

- The **nitpik binary** on your `PATH` (`curl https://nitpik.dev/install.sh | sh`,
  or `cargo install nitpik`). Claude Code plugins don't bundle binaries.
- A **provider configured** for nitpik (BYOM) — e.g. `OPENAI_API_KEY` /
  `ANTHROPIC_API_KEY` and a `.nitpik.toml`, since Claude Code does not expose
  its model to MCP servers (see
  [anthropics/claude-code#1785](https://github.com/anthropics/claude-code/issues/1785)).
- An **active nitpik subscription** (editor/agent reviews are gated).

## Install

Via a plugin marketplace, or for a one-off without the plugin:

```bash
claude mcp add nitpik -- nitpik serve mcp
```

Then ask Claude to "review my changes with nitpik".
