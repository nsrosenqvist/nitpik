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
| `constants.rs` | Centralized app name, config paths, env var names, and URLs — a rename only requires changing this file |
| `cli/` | clap arg parsing, subcommands (`review`, `profiles`, `validate`, `cache`, `license`), CLI entry point wiring |
| `config/` | `.nitpik.toml` loading, env var resolution, config layering (CLI → env → repo config → global config → defaults) |
| `diff/` | Git CLI wrapper, unified diff parsing, file scanning, chunk splitting |
| `context/` | Baseline context: full file loading, project doc detection (supports `--no-project-docs` and `--exclude-doc`) |
| `agents/` | Built-in profiles (`backend`, `frontend`, `architect`, `security`), markdown+YAML parser, auto-profile selection, tag-based profile resolution |
| `providers/` | `ReviewProvider` trait, rig-core multi-provider integration (Anthropic, OpenAI, Gemini, Cohere, DeepSeek, xAI, Groq, Perplexity, OpenAI-compatible) |
| `tools/` | Agentic tools: `ReadFileTool`, `SearchTextTool`, `ListDirectoryTool`, `CustomCommandTool` (user-defined CLI tools from profile frontmatter), `ToolCallLog` (audit log for tool invocations) |
| `orchestrator/` | Parallel review execution, prompt construction, deduplication |
| `output/` | `OutputRenderer` trait + format implementations (terminal, JSON, GitHub, GitLab, Bitbucket, Forgejo) |
| `security/` | Secret scanner, vendored gitleaks rules, entropy checks, redaction |
| `cache/` | Content-hash cache, filesystem storage, branch-scoped sidecar metadata for prior findings |
| `models/` | Shared types: `Finding`, `Severity`, `FileDiff`, `AgentDefinition`, `ReviewConfig`, `ReviewContext`, etc. |
| `license/` | Offline Ed25519 license key verification, expiry checks |
| `progress/` | Live terminal progress display — spinners, status tracking per file×agent task, suppressed with `--quiet` |
| `telemetry/` | Anonymous fire-and-forget heartbeat POST per review run, disabled with `--no-telemetry` or `NITPIK_TELEMETRY=false` |

### Key Principles

- **Modules communicate through `models/`** — import shared types from `models/`, not from sibling module internals.
- **`main.rs` is the composition root** — it wires modules together. The orchestrator coordinates execution; everything else is a leaf.
- **Async-first** — all I/O uses `tokio`. Parallel work uses `JoinSet` with a semaphore for concurrency control. Git is invoked via `tokio::process::Command`.
- **Single source of truth for names and paths** — `constants.rs` centralizes the app name, config filenames, env var names, and URLs. Use those constants instead of hard-coding strings.

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

### Spelling

Use **American English** throughout: code comments, doc comments, agent profile prompts, and documentation. For example: `sanitization` not `sanitisation`, `behavior` not `behaviour`, `specialized` not `specialised`.

### Enums & Constants Over Magic Strings

Prefer enums and constants over raw string literals. Variant names, format identifiers, provider names, severity levels, and similar fixed sets should be modeled as enums with `Display`/`FromStr` impls (or `strum` derives) rather than compared as ad-hoc strings. For application-wide names, paths, and env vars, use the constants defined in `constants.rs`.

### Dependencies

Keep the dependency tree lean — binary size and compile time matter for a CLI tool. Justify any new crate before adding it. Prefer stdlib when reasonable.

### Configuration & Environment Variables

**Config priority (highest wins):**

1. CLI flags
2. Environment variables
3. `.nitpik.toml` in repo root
4. `~/.config/nitpik/config.toml` (global)
5. Built-in defaults

**Key env vars** (all prefixed `NITPIK_`):

| Variable | Purpose |
|---|---|
| `NITPIK_PROVIDER` | LLM provider name |
| `NITPIK_MODEL` | Model identifier |
| `NITPIK_API_KEY` | API key (falls back to provider-specific vars like `ANTHROPIC_API_KEY`) |
| `NITPIK_BASE_URL` | Custom API base URL (for OpenAI-compatible endpoints) |
| `NITPIK_LICENSE_KEY` | Commercial license key |
| `NITPIK_TELEMETRY` | Set `false` to disable telemetry |

### Testing

- **Unit tests**: in-file `#[cfg(test)] mod tests` blocks.
- **Integration tests**: `tests/` directory. Mock the LLM provider — never make real API calls in CI.
- **Snapshot tests**: expected output files in `tests/fixtures/` for output renderers.
- **Edge cases to cover**: empty diffs, binary files, renames, permission changes, provider failures.
- **Test runner**: use [`cargo-nextest`](https://nexte.st/) instead of `cargo test`. It runs each test as a separate process and parallelises across all test binaries simultaneously, which is significantly faster. Install with `cargo install cargo-nextest --locked`, then run `cargo nextest run`. CI should use `cargo nextest run` as well.
- **Slow tests**: the `security::rules::tests::default_rules_*` tests compile 219 gitleaks regexes (~25 s). They share a `LazyLock` within each binary so the cost is paid once per process, but they will always dominate wall-clock time.

### Documentation

- Public items get `///` doc comments.
- Each `mod.rs` gets a `//!` module-level doc explaining its purpose.
- Keep this file updated when adding modules or changing architecture.

### Version Management

- **`Cargo.toml` keeps a dev placeholder version** (e.g. `0.1.0`). CI patches it from the git tag before building.
- **`build.rs`** emits `GIT_SHA`, `BUILD_DATE`, and `TARGET` as compile-time env vars. `CARGO_PKG_VERSION` comes from Cargo (patched in CI).
- **`constants.rs`** exposes `VERSION`, `GIT_SHA`, `BUILD_DATE`, `TARGET`, and `USER_AGENT` as the single source of truth. Other modules import from there.
- To **release a new version**: push a `v*` tag (e.g. `v0.2.0`). The release workflow patches `Cargo.toml`, builds cross-platform binaries, publishes to crates.io, and pushes a Docker image.

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

**Frontmatter fields**: `name`, `description`, `model` (optional), `tags`, `tools` (optional), `agentic_instructions` (optional), `environment` (optional).

- The `agentic_instructions` field contains tool-usage guidance that is **only** injected in agentic mode (when the LLM has access to tools). Keep it out of the main system prompt body so non-agentic reviews aren't confused by references to tools.
- The `environment` field lists env var names (or prefix globs like `AWS_*`) that custom command tools are allowed to inherit. By default, all LLM API keys and nitpik secrets are stripped from subprocess environments. See **Environment Sanitization** below.
- Use `tags` to describe the profile's focus areas. Tags serve double duty: users can select profiles with `--tag`, and the orchestrator uses them to build a dynamic coordination note telling each reviewer what the sibling reviewers cover.
- Do **not** hardcode references to other profile names in the system prompt body. When multiple agents run in parallel, the orchestrator injects a coordination note listing sibling reviewers and their tags automatically.

### Add an Agentic Tool

1. Create `src/tools/my_tool.rs`, implement rig-core's `Tool` trait
2. Register in `src/tools/mod.rs`
3. Add to agent construction in `src/providers/rig.rs`
4. Document the tool in agent system prompts

### Add a Custom Command Tool (in an Agent Profile)

Users can define CLI tools directly in an agent profile's YAML frontmatter — no Rust code needed:

```yaml
tools:
  - name: run_tests
    description: Run the project's test suite
    command: cargo test
    parameters:
      - name: filter
        type: string
        description: Optional test name filter
        required: false
```

At runtime each entry becomes a `CustomCommandTool` the LLM can invoke. Commands are sandboxed to the repo root with a 120 s timeout and 256 KB output cap.

#### Resource Limits

Every custom command subprocess runs behind `ulimit` guards that constrain resource consumption:

| Limit | Value | `ulimit` flag |
|---|---|---|
| Virtual memory | 1 GB | `-v 1048576` |
| File write size | 100 MB | `-f 204800` |

These are applied as a shell preamble (`ulimit ... 2>/dev/null; <command>`) so unsupported limits on a given platform are silently ignored (e.g. `ulimit -v` on macOS Apple Silicon). Combined with the existing 120 s timeout and 256 KB output cap, this prevents runaway commands from exhausting host resources.

> **Note:** `ulimit -u` (max user processes) is intentionally omitted — it caps the *per-user* process count, not a per-subprocess-tree count, so on busy systems or in parallel test runners the existing process count can already exceed the limit, preventing the subprocess from forking at all. Only cgroups can truly scope a process-tree limit, and those require root.

#### Unknown Parameter Handling

If the LLM passes parameter names that are not declared in the tool definition, they are silently ignored (they never reach the command string). Unknown parameter names are logged in the tool-call audit entry for observability — e.g. `exit 0, 537B, ignored unknown params: rogue_param`.

#### Environment Sanitization

Custom command subprocesses inherit the parent environment **minus** all sensitive variables listed in `constants::SENSITIVE_ENV_VARS` (LLM API keys like `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc., plus `NITPIK_API_KEY` and `NITPIK_LICENSE_KEY`). This prevents accidental key leakage to user-defined commands.

If a custom tool needs specific env vars (e.g. to authenticate with Jira, Docker, or AWS), declare them in the profile's `environment` frontmatter field:

```yaml
environment:
  - JIRA_TOKEN
  - AWS_*          # prefix glob — matches AWS_REGION, AWS_SECRET_ACCESS_KEY, etc.
  - DOCKER_HOST
tools:
  - name: deploy_check
    description: Check deployment status
    command: curl -sH "Authorization: Bearer $JIRA_TOKEN" https://jira.example.com/status
```

Exact names and prefix globs (ending with `*`) are supported. Variables not in the sensitive list pass through unconditionally — you only need `environment` entries to re-allow variables that would otherwise be stripped.

### Tool-Call Audit Log

Every agentic tool invocation (built-in and custom) is recorded in a process-global `ToolCallLog` (`src/tools/mod.rs`). Each entry captures:

- Tool name, argument summary, result summary, and wall-clock duration.

The progress display (`src/progress/mod.rs`) drains this log at the end of a review run and prints a compact summary showing what the LLM explored:

```
  ▸ 5 tool calls
    → read_file src/main.rs (1.2KB, 3ms)
    → search_text "fn process" (3 results, 45ms)
    → run_tests --filter auth (exit 0, 2.1s)
```

This is shown after the file-status summary and before the findings count. It is suppressed with `--quiet`.

### Add LLM Provider Support

If rig-core supports it, add the provider name to config resolution in `src/config/loader.rs` and the provider construction in `src/providers/rig.rs`. Otherwise, implement a rig-core provider adapter or extend `ReviewProvider`.

### `tools/` Directory (Project Root)

The top-level `tools/` directory contains standalone Rust utilities for license key management (`keygen.rs`, `issue_license.rs`). These are not part of the main binary.

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

- Run the full test suite (`cargo nextest run`) and confirm all tests pass.
- If you added new functionality, add tests for it.
- If you changed a public API, update callers and tests accordingly.
- If you made architectural changes (new modules, new traits, changed module boundaries), update this file and `README.md`.
- If you changed CLI flags or config options, update `README.md`.
- Verify zero compiler warnings (`cargo build` should produce no `warning:` lines).
