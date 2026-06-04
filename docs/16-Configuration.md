# Configuration

nitpik is configured through a layered system — CLI flags, environment variables, config files, and built-in defaults. Each layer overrides the one below it.

---

## Configuration Priority

From highest to lowest priority:

1. **CLI flags** — always win
2. **Environment variables** — override config files
3. **`.nitpik.toml`** in repo root — project-level defaults
4. **`~/.config/nitpik/config.toml`** — global user defaults
5. **Built-in defaults** — fallback values

## Project Config (`.nitpik.toml`)

Drop this in your repository root to set defaults for your team:

```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-5-20250929"
# base_url = "https://custom-endpoint.example.com/v1"  # for openai-compatible

[review]
default_profiles = ["backend", "security"]
fail_on = "warning"

[review.agentic]
enabled = false
max_turns = 10
max_tool_calls = 10

[review.context]
max_file_lines = 1000
surrounding_lines = 100
rolling_summary = false

[secrets]
enabled = false
severity = "warning"

[threats]
enabled = false

[telemetry]
enabled = true
```

## Global Config (`~/.config/nitpik/config.toml`)

Same format as `.nitpik.toml`. Use this for personal defaults that apply across all repositories — like your preferred provider and model.

The project config overrides the global config, so teams can set project-level standards that take precedence over individual preferences.

## Config Sections Reference

### `[provider]`

| Key | Type | Default | Description |
|---|---|---|---|
| `name` | string | `"anthropic"` | LLM provider. One of: `anthropic`, `openai`, `gemini`, `cohere`, `deepseek`, `xai`, `groq`, `perplexity`, `openai-compatible`. |
| `model` | string | *(per-provider)* | Model identifier passed to the provider. If omitted, nitpik uses a [sensible default for each provider](03-Providers#default-models). |
| `base_url` | string | *(none)* | Custom API endpoint. Required for `openai-compatible`, optional for others. |
| `api_key` | string | *(none)* | API key. Prefer env vars over config files for secrets. |

### `[review]`

| Key | Type | Default | Description |
|---|---|---|---|
| `default_profiles` | array | `["auto"]` | Profiles used when `--profile` is not specified on the CLI. The CLI default is `auto` (heuristic selection); set explicit names here to opt out. |
| `fail_on` | string | `"error"` | Fail-on severity threshold. One of: `error`, `warning`, `info`. nitpik exits non-zero if any finding meets this threshold. Use `--no-fail` on the CLI to disable. |
| `audit_log` | string | *(none)* | Path to write the per-run JSON audit log. When set, nitpik captures per-task status, tool calls, retries, token usage, critic decisions, and final findings. CLI flag `--audit-log` and env var `NITPIK_AUDIT_LOG` take precedence. |

### `[review.agentic]`

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Enable agentic mode by default. Equivalent to always passing `--agent`. |
| `max_turns` | integer | `10` | Max LLM round-trips per file×agent task. Higher values allow deeper exploration but increase cost. |
| `max_tool_calls` | integer | `10` | Max tool invocations per file×agent task. Caps total tool calls regardless of turns. |

### `[review.context]`

| Key | Type | Default | Description |
|---|---|---|---|
| `max_file_lines` | integer | `1000` | Files with more lines than this get hunk excerpts instead of full content. Larger values give the LLM more context but increase token cost. |
| `surrounding_lines` | integer | `100` | Number of context lines around each diff hunk for large files. Only applies when the file exceeds `max_file_lines`. |
| `rolling_summary` | boolean | `false` | Generate a functional summary of the whole change (one extra LLM call per run) and feed it into every reviewer's context. Persisted per branch in the cache, so on re-runs it accumulates context across pushes. Also enabled per-run with `--pr-summary`. |

### `[secrets]`

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Enable secret scanning by default. Equivalent to always passing `--scan-secrets`. Adds ~3-5s startup time. |
| `severity` | string | `"warning"` | Severity level for detected secrets. One of: `error`, `warning`, `info`. Set `error` to block merges; set `info` for legacy codebases. CLI flag: `--secrets-severity`. |

### `[threats]`

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Enable threat scanning by default. Equivalent to always passing `--scan-threats`. |
| `additional_rules` | string | *(none)* | Path to additional threat rules TOML file. Loaded alongside the 44 built-in rules. |

### `[license]`

| Key | Type | Default | Description |
|---|---|---|---|
| `key` | string | *(none)* | Commercial license key (format `nkp_live_…`). Set by `nitpik license activate`. Can also use `NITPIK_LICENSE_KEY` env var. The CLI exchanges this key with `nitpik.dev` for a short-lived entitlement, cached at `~/.config/nitpik/entitlement.json`. See [Licensing](20-Licensing). |

### `[telemetry]`

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `true` | Enable anonymous usage telemetry. Set `false` to disable. Can also use `NITPIK_TELEMETRY=false` env var or `--no-telemetry` flag. |

## Environment Variables

### Provider & Model

| Variable | Description |
|---|---|
| `NITPIK_PROVIDER` | LLM provider name (overrides `[provider].name`) |
| `NITPIK_MODEL` | Model identifier (overrides `[provider].model`) |
| `NITPIK_API_KEY` | Universal API key fallback — used when no provider-specific key is set |
| `NITPIK_BASE_URL` | Custom API endpoint (overrides `[provider].base_url`) |

### Provider-Specific API Keys

nitpik checks for a provider-specific key first, then falls back to `NITPIK_API_KEY`:

| Variable | Provider |
|---|---|
| `ANTHROPIC_API_KEY` | Anthropic |
| `OPENAI_API_KEY` | OpenAI and openai-compatible |
| `GEMINI_API_KEY` | Google Gemini |
| `COHERE_API_KEY` | Cohere |
| `DEEPSEEK_API_KEY` | DeepSeek |
| `XAI_API_KEY` | xAI (Grok) |
| `GROQ_API_KEY` | Groq |
| `PERPLEXITY_API_KEY` | Perplexity |

### CI Platform Tokens

| Variable | Purpose |
|---|---|
| `BITBUCKET_TOKEN` | Bitbucket access token for `--format bitbucket` (optional inside Bitbucket Pipelines) |
| `FORGEJO_TOKEN` | Forgejo/Gitea API token for `--format forgejo` |

### Other

| Variable | Description |
|---|---|
| `NITPIK_LICENSE_KEY` | Commercial license key (`nkp_live_…`). Exchanged with nitpik.dev for a signed entitlement on first use, then cached. |
| `NITPIK_OFFLINE_TOKEN` | Pre-signed entitlement JWT for air-gapped CI. When set, bypasses the network exchange entirely. Generate one at [nitpik.dev/account](https://nitpik.dev/account). |
| `NITPIK_API_URL` | Override the nitpik.dev origin used for entitlement fetches (defaults to `https://nitpik.dev`; useful for staging). |
| `NITPIK_TELEMETRY` | Set `false` to disable telemetry |
| `NITPIK_AUDIT_LOG` | Path to write a per-run JSON audit log (equivalent to `--audit-log`) |

## Related Pages

- [LLM Providers](03-Providers) — provider setup details
- [CLI Reference](18-CLI-Reference) — every command and flag
- [CI/CD Integration](17-CI-Integration) — configuration for CI environments
