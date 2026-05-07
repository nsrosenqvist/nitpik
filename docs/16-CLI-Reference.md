# CLI Reference

Complete reference for every nitpik command and flag.

---

## Global Flags

These flags apply to all subcommands:

| Flag | Description |
|---|---|
| `--no-telemetry` | Disable anonymous usage telemetry for this run. |
| `--version` | Print version string and exit. |
| `--help` | Print help for the current command. |

## `nitpik review`

Run a code review.

### Input Flags (exactly one required)

| Flag | Default | Description |
|---|---|---|
| `--diff-base <REF>` | — | Branch, tag, or commit to diff against using `git diff`. |
| `--diff-file <PATH>` | — | Pre-computed unified diff file. |
| `--diff-stdin` | `false` | Read unified diff from stdin. |
| `--scan <PATH>` | — | Review a file or directory directly (no git required). |

### Repository

| Flag | Default | Description |
|---|---|---|
| `--path <DIR>` | `.` | Repository or working directory path. |

### Profile Selection

| Flag | Default | Description |
|---|---|---|
| `--profile <NAMES>` | `general` | Comma-separated list of profile names, file paths, or `auto`. Built-in: `backend`, `frontend`, `architect`, `security`, `general`. |
| `--profile-dir <DIR>` | — | Directory to resolve bare profile names from. |
| `--tag <TAGS>` | — | Comma-separated tags. All profiles (built-in and custom) whose tags match are included. Combines with `--profile`. |
| `--auto-mode <MODE>` | `hybrid` | How `--profile auto` picks reviewers: `heuristic` (rules only, no LLM call), `llm` (always ask the model), `hybrid` (heuristics first, fall back to LLM when inconclusive). |
| `--multi-wave` | `false` | Run reviewers in waves. Profiles whose frontmatter declares `wave: 2` run after wave 1 and receive the wave-1 findings as context. Capped at 2 waves. |

### Output

| Flag | Default | Description |
|---|---|---|
| `--format <FORMAT>` | `terminal` | Output format: `terminal`, `json`, `github`, `gitlab`, `bitbucket`, `checkstyle`, `forgejo`. |
| `--fail-on <SEVERITY>` | `error` | Exit non-zero if any finding meets this severity: `error`, `warning`, `info`. |
| `--no-fail` | `false` | Never exit non-zero on findings, even when `--fail-on` or config is set. |
| `-q`, `--quiet` | `false` | Suppress banner, progress display, and informational messages. Only findings and errors are shown. |
| `--no-tokens` | `false` | Suppress the per-run token usage summary printed after the review. Only affects terminal output. |

### Verification

| Flag | Default | Description |
|---|---|---|
| `--verify` | `false` | Run a critic pass after the main review that votes keep/drop on each finding to suppress probable false positives. Adds one extra LLM call per run with findings. |
| `--show-dropped` | `false` | Print findings the critic dropped (with rationale) to stderr. No effect without `--verify`. |

### Agentic Mode

| Flag | Default | Description |
|---|---|---|
| `--agent` | `false` | Enable agentic mode — lets the LLM use tools to explore the codebase. |
| `--max-turns <N>` | `10` | Max LLM round-trips (tool call → response) per file×agent task. |
| `--max-tool-calls <N>` | `10` | Max tool invocations per file×agent task. |

### Secret Scanning

| Flag | Default | Description |
|---|---|---|
| `--scan-secrets` | `false` | Enable secret detection and redaction before LLM calls. |
| `--secrets-rules <PATH>` | — | Additional gitleaks-format TOML rules file. |
| `--secrets-severity <LEVEL>` | `warning` | Severity level for detected secrets (`error`, `warning`, or `info`). Set `error` to block merges on secrets; set `info` for legacy codebases. |

### Threat Scanning

| Flag | Default | Description |
|---|---|---|
| `--scan-threats` | `false` | Enable threat pattern detection (obfuscation, dangerous APIs, supply chain, backdoors) with optional LLM triage. |
| `--threat-rules <PATH>` | — | Additional threat rules file (TOML format). Loaded alongside the 44 built-in rules. |

### Caching

| Flag | Default | Description |
|---|---|---|
| `--no-cache` | `false` | Disable result caching. Every file is re-reviewed. |
| `--no-prior-context` | `false` | Skip injecting previous findings into the prompt on cache invalidation. |
| `--max-prior-findings <N>` | unlimited | Cap the number of prior findings included in the prompt. |

### Context

| Flag | Default | Description |
|---|---|---|
| `--no-project-docs` | `false` | Skip auto-detected project documentation files. |
| `--exclude-doc <NAMES>` | — | Comma-separated filenames to exclude from project docs (e.g. `AGENTS.md,CONTRIBUTING.md`). |
| `--no-commit-context` | `false` | Skip injecting commit summaries into the review prompt. Only affects `--diff-base` mode. |

### Performance

| Flag | Default | Description |
|---|---|---|
| `--max-concurrent <N>` | `5` | Max concurrent LLM calls. |
| `--timeout <SECONDS>` | `300` | Per-attempt timeout for each file × agent review call (wraps the whole agentic loop, including all turns and tool roundtrips). On timeout the call is treated as a retryable error; each retry gets a fresh budget. Set to `0` to disable. |

### Audit Log

| Flag | Default | Description |
|---|---|---|
| `--audit-log <PATH>` | — | Write a structured JSON audit log to `PATH` after the run. Records per-task status, tool calls, retries, token usage, critic decisions, and final findings. Useful as a CI build artifact for after-the-fact debugging. Also configurable via `NITPIK_AUDIT_LOG` env var or `[review].audit_log` in `.nitpik.toml`. |

---

## `nitpik profiles`

List all available profiles (built-in and custom).

| Flag | Description |
|---|---|
| `--profile-dir <DIR>` | Directory to scan for additional custom profiles. |

---

## `nitpik validate <FILE>`

Validate a custom agent profile definition. Checks YAML frontmatter structure, required fields, and tool definitions.

**Argument:** path to the profile Markdown file.

---

## `nitpik cache`

Manage the result cache.

### Subcommands

| Subcommand | Description |
|---|---|
| `nitpik cache clear` | Remove all cached review results and sidecar metadata. |
| `nitpik cache stats` | Show cache entry count and total size. |
| `nitpik cache path` | Print the cache directory path. |

---

## `nitpik license`

Manage the commercial license key.

### Subcommands

| Subcommand | Description |
|---|---|
| `nitpik license activate <KEY>` | Store a license key in `~/.config/nitpik/config.toml`. |
| `nitpik license status` | Show current license status (customer, expiry). |
| `nitpik license deactivate` | Remove the license key from global config. |

---

## `nitpik update`

Update nitpik to the latest release from GitHub.

| Flag | Description |
|---|---|
| `--force` | Re-download even if already on the latest version. |

Downloads the release archive for your platform, verifies its SHA256 checksum, and atomically replaces the running binary.

---

## `nitpik version`

Print detailed build metadata: version, git commit, build date, and target triple.

```
nitpik 0.2.0
commit:     a1b2c3d
built:      2026-02-14
target:     x86_64-unknown-linux-gnu
```

---

## Related Pages

- [Configuration](14-Configuration) — config files and environment variables
- [Quick Start](02-Quick-Start) — get started quickly
