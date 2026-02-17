# Agentic Mode

In standard mode, the LLM reviews only the diff and context provided in the prompt. In agentic mode, the LLM can actively explore your codebase — reading files, searching for patterns, listing directories, and running custom commands — to verify its findings before reporting them.

---

## Enabling Agentic Mode

```bash
nitpik review --diff-base main --agent
```

Or enable it by default in `.nitpik.toml`:

```toml
[review.agentic]
enabled = true
```

## When to Use Agentic Mode

**Use agentic mode when:**
- Your changes interact with code in other files (e.g., interface changes, API contracts)
- You want the reviewer to verify assumptions before reporting (e.g., "is this error actually handled elsewhere?")
- Your custom profiles define tools (test runners, linters, deployment checks)

**Use standard mode when:**
- You want faster, cheaper reviews (no extra LLM turns for tool calls)
- The changes are self-contained within the diff
- You're optimizing for API cost

Agentic reviews use more LLM tokens (each tool call is an additional turn) but produce fewer false positives because the LLM can verify its reasoning.

## Built-in Tools

Every agentic review has access to three built-in tools:

### `read_file`

Reads the contents of a file from the repository. The LLM uses this to examine functions, types, or modules referenced in the diff.

### `search_text`

Searches for a text pattern across the codebase. The LLM uses this to find usages of a changed function, check whether an issue is handled elsewhere, or trace data flow.

### `list_directory`

Lists the contents of a directory. The LLM uses this to understand module structure and navigate the codebase.

## Custom Tools

Profiles can define additional CLI tools. See [Custom Profiles — Custom Agentic Tools](06-Custom-Profiles#custom-agentic-tools) for the full format.

```bash
nitpik review --diff-base main --profile ./test-aware-reviewer.md --agent
```

## Controlling Resource Usage

### Max Turns

Limits how many LLM round-trips (tool call → response → next call) are allowed per file×agent task:

```bash
nitpik review --diff-base main --agent --max-turns 5
```

Defaults to `10`. Lower values reduce cost and latency. If the LLM reaches the limit, it returns whatever findings it has at that point.

### Max Tool Calls

Limits the total number of tool invocations per file×agent task:

```bash
nitpik review --diff-base main --agent --max-tool-calls 5
```

Defaults to `10`. This caps the total number of `read_file`, `search_text`, etc. calls within a single task, preventing runaway exploration.

### Config File

```toml
[review.agentic]
enabled = true
max_turns = 10
max_tool_calls = 10
```

## Tool-Call Audit Log

Every tool invocation is recorded and displayed at the end of the review (unless `--quiet` is set):

```
  ▸ 5 tool calls
    → read_file src/main.rs (1.2KB, 3ms)
    → search_text "fn process" (3 results, 45ms)
    → run_tests --filter auth (exit 0, 2.1s)
```

This shows what the LLM explored, helping you understand its reasoning and verify it didn't go off-track.

## Agentic Context

In agentic mode, the LLM receives additional context beyond the standard diff and file content:

- **Repository structure** — a listing of the repo root so the LLM knows what files and directories exist.
- **Sibling changed files** — a list of other files in the review, so the LLM can cross-reference related changes.
- **Tool usage guidance** — instructions on how to use each available tool effectively.

Profile-specific `agentic_instructions` from the frontmatter are also injected, giving each profile tailored guidance for tool usage. These instructions are *not* included in standard mode, so they won't confuse the LLM when tools aren't available.

## Related Pages

- [Custom Profiles](06-Custom-Profiles) — define custom tools and agentic instructions
- [Reviewer Profiles](05-Reviewer-Profiles) — choosing profiles
- [How Reviews Work](09-How-Reviews-Work) — the full review pipeline
- [Configuration](13-Configuration) — agentic config options
