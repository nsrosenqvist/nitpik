# AGENTS.md — Contributor Guide

Guidelines and conventions for contributing to **nitpik**, an AI-powered code review CLI built in Rust.

## Project Overview

nitpik is a single-binary Rust CLI for local use and CI pipelines. It accepts diffs (file, git ref, or directory scan), routes them through configurable reviewer agents, and outputs structured findings in multiple formats.

## Architecture

### Extension Points

The codebase uses traits at every point where multiple implementations are expected:

- **`OutputRenderer`** — rendering findings to different formats
- **`ReviewProvider`** — LLM interaction (currently backed by rig-core)
- **`Tool`** (rig-core) — agentic tools for codebase exploration

Extend the system by implementing a trait, not by modifying existing implementations.

### Module Layout

| Module | Purpose |
|---|---|
| `cli/` | clap arg parsing, CLI entry point wiring |
| `config/` | `.nitpik.toml` loading, env var resolution, config layering |
| `diff/` | Git CLI wrapper, unified diff parsing, file scanning, chunk splitting |
| `context/` | Baseline context: full file loading, project doc detection |
| `agents/` | Built-in profiles, markdown+YAML parser, auto-profile selection |
| `providers/` | `ReviewProvider` trait, rig-core multi-provider integration |
| `tools/` | Agentic tools: `ReadFileTool`, `SearchTextTool`, `ListDirectoryTool` |
| `orchestrator/` | Parallel review execution, prompt construction, deduplication |
| `output/` | `OutputRenderer` trait + format implementations |
| `security/` | Secret scanner, vendored gitleaks rules, entropy checks, redaction |
| `cache/` | Content-hash cache, filesystem storage |
| `models/` | Shared types: `Finding`, `Severity`, `FileDiff`, `AgentDefinition`, etc. |

### Key Principles

- **Modules communicate through `models/`** — import shared types from `models/`, not from sibling module internals.
- **`main.rs` is the composition root** — it wires modules together. The orchestrator coordinates execution; everything else is a leaf.
- **Async-first** — all I/O uses `tokio`. Parallel work uses `JoinSet` with a semaphore for concurrency control. Git is invoked via `tokio::process::Command`.

## Conventions

### Error Handling

- Library code (`src/` except `main.rs`): `thiserror` enums, one per module.
- CLI boundary (`main.rs`): `anyhow` for ergonomic propagation.
- No `.unwrap()` outside tests. Use `.expect("reason")` only for proven invariants.
- Prefer `?` over `match` when the error should propagate.

### Naming

- Files: `snake_case.rs`
- Types: `PascalCase`
- Functions/variables: `snake_case`
- Constants: `SCREAMING_SNAKE_CASE`
- Traits: describe a capability (`OutputRenderer`, `ReviewProvider`)

### Dependencies

Keep the dependency tree lean — binary size and compile time matter for a CLI tool. Justify any new crate before adding it. Prefer stdlib when reasonable.

### Testing

- **Unit tests**: in-file `#[cfg(test)] mod tests` blocks.
- **Integration tests**: `tests/` directory. Mock the LLM provider — never make real API calls in CI.
- **Snapshot tests**: expected output files in `tests/fixtures/` for output renderers.
- **Edge cases to cover**: empty diffs, binary files, renames, permission changes, provider failures.

### Documentation

- Public items get `///` doc comments.
- Each `mod.rs` gets a `//!` module-level doc explaining its purpose.
- Keep this file updated when adding modules or changing architecture.

### Git

- Imperative mood, <72 char subject line.
- One logical change per commit.

## How To

### Add an Output Format

1. Create `src/output/my_format.rs`, implement `OutputRenderer`
2. Register in `src/output/mod.rs`
3. Add the format variant to the CLI enum in `src/cli/args.rs`
4. Add snapshot tests in `tests/`

### Add a Built-In Agent Profile

1. Create `src/agents/builtin/my_profile.md` (YAML frontmatter + system prompt)
2. Register with `include_str!` in `src/agents/builtin/mod.rs`

### Add an Agentic Tool

1. Create `src/tools/my_tool.rs`, implement rig-core's `Tool` trait
2. Register in `src/tools/mod.rs`
3. Add to agent construction in `src/providers/rig.rs`
4. Document the tool in agent system prompts

### Add LLM Provider Support

If rig-core supports it, add the provider name to config resolution. Otherwise, implement a rig-core provider adapter or extend `ReviewProvider`.

## Agent Workflow

Follow these practices when working on this codebase as an AI coding agent.

### Before You Start

- Read this file and the module layout to orient yourself.
- Use the codebase — search, read files, check types — before making assumptions about how something works.
- When a task spans multiple modules, plan the full set of changes before editing.

### While Working

- Build after every meaningful change (`cargo build`). Fix errors before moving on.
- Keep the compiler warning-free. Do not introduce new warnings.
- Follow existing patterns in the module you're editing — don't invent new conventions locally.

### Before You're Done

- Run the full test suite (`cargo test`) and confirm all tests pass.
- If you added new functionality, add tests for it.
- If you changed a public API, update callers and tests accordingly.
- If you made architectural changes (new modules, new traits, changed module boundaries), update this file and `README.md`.
- If you changed CLI flags or config options, update `README.md`.
- Verify zero compiler warnings (`cargo build` should produce no `warning:` lines).
