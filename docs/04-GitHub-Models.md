# GitHub Models

[GitHub Models](https://docs.github.com/en/github-models) is GitHub's hosted inference service. It exposes an OpenAI-compatible Chat Completions API at `https://models.github.ai/inference`, fronts models from OpenAI, DeepSeek, Meta, Microsoft, Mistral, and others, and — crucially for code review in CI — authenticates with the `GITHUB_TOKEN` that GitHub Actions provides automatically.

nitpik ships a first-class `github` provider for this endpoint. In a GitHub Actions workflow, the entire setup is one env var and one extra permission.

---

## GitHub Actions (primary use case)

This is what GitHub Models is uniquely good for: no secrets, no API key management, no out-of-band onboarding. The job's built-in `GITHUB_TOKEN` is the API key, and the only thing you change in your workflow is the `permissions` block.

```yaml
name: Review

on:
  pull_request:

permissions:
  contents: read
  pull-requests: write
  models: read              # required for GitHub Models

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: nsrosenqvist/nitpik@v3
        env:
          NITPIK_PROVIDER: github
          # GITHUB_TOKEN is exported automatically by Actions; nitpik picks it up.
```

`models: read` is the new permission GitHub introduced for Models access. Without it the workflow's `GITHUB_TOKEN` will be issued without that scope and the inference call will return `403`. See [CI/CD Integration](17-CI-Integration) for the wider pipeline (output formats, PR annotations, fail-on policies).

## Local CLI use

For running nitpik against a diff on your machine, create a [fine-grained personal access token](https://github.com/settings/personal-access-tokens) with the `Models: Read-only` permission, then:

```bash
export NITPIK_PROVIDER=github
export GITHUB_TOKEN=ghp_...
nitpik review --base main
```

If you already have the [`gh` CLI](https://cli.github.com/) authenticated, `gh auth token` will print a usable token. Note that `gh`'s default token does not include the `models:read` scope — you may need to re-auth with `gh auth refresh --scopes 'models:read'` or use a dedicated PAT.

## Model name format

GitHub Models requires model IDs in `{publisher}/{model}` form. Examples:

- `openai/gpt-4.1` (nitpik's default)
- `openai/gpt-4.1-mini`
- `deepseek/DeepSeek-V3-0324`
- `meta/Llama-3.3-70B-Instruct`
- `microsoft/Phi-4`
- `mistral-ai/Mistral-Large-2411`

Browse the full catalogue at [github.com/marketplace/models](https://github.com/marketplace/models). Override the default by setting `NITPIK_MODEL`:

```bash
export NITPIK_MODEL=deepseek/DeepSeek-V3-0324
```

Or in `.nitpik.toml`:

```toml
[provider]
name = "github"
model = "openai/gpt-4.1"
```

> **Tip:** Code review benefits from strong reasoning. `openai/gpt-4.1`, the larger DeepSeek variants, and Llama 3.3 70B all produce solid findings. Smaller models are faster but miss subtle issues.

## Rate limits and the free tier

GitHub Models has a free tier with per-model-class rate limits. Limits are tracked per user (or per organization for org-attributed requests) and reset on a rolling window.

What this means for nitpik in practice:

- **Single-file or small diffs**: free tier is plenty.
- **Large multi-file diffs with `auto` profile selection**: you may hit the per-minute limit. nitpik will surface `429`s as retryable errors and back off, but if the daily cap is exhausted the review will fail.
- **Mitigations**: narrow `--profile` to one or two specialists; switch to a cheaper model class; flip to a paid tier; or fall back to another [LLM Provider](03-Providers) for heavy runs.

GitHub's current limits are documented at [docs.github.com/en/github-models](https://docs.github.com/en/github-models). Treat them as a moving target.

## Organization-attributed billing

To attribute usage and rate limits to a GitHub organization rather than to the calling user, point `NITPIK_BASE_URL` at the org-scoped endpoint:

```bash
export NITPIK_PROVIDER=github
export NITPIK_BASE_URL=https://models.github.ai/orgs/YOUR-ORG/inference
export GITHUB_TOKEN=ghp_...
```

The token must be authorized for that organization. In GitHub Actions, the workflow's `GITHUB_TOKEN` is already scoped to the running repo's owner — set the base URL only if you want to centralize billing under a different org.

## Configuration file

Everything above can live in `.nitpik.toml` instead of env vars:

```toml
[provider]
name = "github"
model = "openai/gpt-4.1"
# base_url = "https://models.github.ai/orgs/YOUR-ORG/inference"   # optional
# api_key  = "..."   # not recommended — prefer GITHUB_TOKEN env var
```

See [Configuration](16-Configuration) for the full layering order (CLI → env → repo `.nitpik.toml` → user `config.toml` → defaults).

## Caveats

> **Third-party dependency notice:** GitHub Models is a separate service operated by GitHub, not by nitpik. Available models, rate limits, model name format, and request semantics are all subject to change by GitHub at any time. Before depending on a specific model for production CI, verify it works with the free unlicensed version of nitpik.

- GitHub may deprecate or rename models without notice; pin a specific `NITPIK_MODEL` in `.nitpik.toml` for stability.
- Prompt caching behavior for GitHub Models is not documented upstream. nitpik's run summary may not report cache hits even when the underlying provider is caching.
- The free tier is metered per natural day in UTC. Plan large reviews accordingly.

## Related Pages

- [LLM Providers](03-Providers) — all supported providers and config layering
- [CI/CD Integration](17-CI-Integration) — full GitHub Actions pipeline setup
- [Quick Start](02-Quick-Start) — run your first review
- [Configuration](16-Configuration) — all config options
