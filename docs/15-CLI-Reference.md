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
| `--profile <NAMES>` | `backend` | Comma-separated list of profile names, file paths, or `auto`. Built-in: `backend`, `frontend`, `architect`, `security`. |
| `--profile-dir <DIR>` | — | Directory to resolve bare profile names from. |
| `--tag <TAGS>` | — | Comma-separated tags. All profiles (built-in and custom) whose tags match are included. Combines with `--profile`. |

### Output

| Flag | Default | Description |
|---|---|---|
| `--format <FORMAT>` | `terminal` | Output format: `terminal`, `json`, `github`, `gitlab`, `bitbucket`, `forgejo`. |
| `--fail-on <SEVERITY>` | `error` | Exit non-zero if any finding meets this severity: `error`, `warning`, `info`. |
| `--no-fail` | `false` | Never exit non-zero on findings, even when `--fail-on` or config is set. |
| `-q`, `--quiet` | `false` | Suppress banner, progress display, and informational messages. Only findings and errors are shown. |

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

- [Configuration](13-Configuration) — config files and environment variables
- [Quick Start](02-Quick-Start) — get started quickly
