# LLM Providers

nitpik is bring-your-own-model. You choose the LLM provider and supply your own API key — nitpik never proxies, stores, or meters your API calls.

---

## Supported Providers

| Provider | `NITPIK_PROVIDER` value | Provider-specific env var |
|---|---|---|
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` |
| Azure OpenAI | `azure` | `AZURE_OPENAI_API_KEY` |
| Cohere | `cohere` | `COHERE_API_KEY` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` |
| Galadriel | `galadriel` | `GALADRIEL_API_KEY` |
| GitHub Models | `github` | `GITHUB_TOKEN` |
| Google Gemini | `gemini` | `GEMINI_API_KEY` |
| Groq | `groq` | `GROQ_API_KEY` |
| HuggingFace | `huggingface` | `HUGGINGFACE_API_KEY` |
| Hyperbolic | `hyperbolic` | `HYPERBOLIC_API_KEY` |
| Mira | `mira` | `MIRA_API_KEY` |
| Mistral | `mistral` | `MISTRAL_API_KEY` |
| Moonshot | `moonshot` | `MOONSHOT_API_KEY` |
| Ollama | `ollama` | _(none — runs locally)_ |
| OpenAI | `openai` | `OPENAI_API_KEY` |
| OpenRouter | `openrouter` | `OPENROUTER_API_KEY` |
| Perplexity | `perplexity` | `PERPLEXITY_API_KEY` |
| Together | `together` | `TOGETHER_API_KEY` |
| xAI (Grok) | `xai` | `XAI_API_KEY` |
| OpenAI-compatible | `openai-compatible` | `OPENAI_API_KEY` |

> **Third-party dependency notice:** Provider integrations are powered by a third-party open-source library. This means provider support may change, break, or be removed due to upstream updates outside of nitpik's control. If you are considering a commercial license, we recommend verifying that your provider and model work correctly using the free unlicensed version of nitpik before purchasing. No license key is needed — just install and test with your own API key.

## Basic Setup

Set two environment variables:

```bash
export NITPIK_PROVIDER=anthropic
export ANTHROPIC_API_KEY=sk-ant-...
```

nitpik looks for the provider-specific key first (e.g. `ANTHROPIC_API_KEY`), then falls back to `NITPIK_API_KEY` as a universal alternative:

```bash
export NITPIK_PROVIDER=anthropic
export NITPIK_API_KEY=sk-ant-...       # works for any provider
```

## Choosing a Model

By default, nitpik picks a sensible model for each provider — you only need to set `NITPIK_PROVIDER` and an API key to get started. Override the default with `NITPIK_MODEL`:

```bash
export NITPIK_MODEL=claude-sonnet-4-5-20250929
```

Or in your `.nitpik.toml`:

```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-5-20250929"
```

### Default Models

| Provider | Default model |
|---|---|
| Anthropic | `claude-sonnet-4-5-20250929` |
| Azure OpenAI | `gpt-4o` |
| Cohere | `command-r-plus` |
| DeepSeek | `deepseek-chat` |
| Galadriel | `llama3.1-70b` |
| GitHub Models | `openai/gpt-4.1` |
| Google Gemini | `gemini-2.5-flash` |
| Groq | `llama-3.3-70b-versatile` |
| HuggingFace | `meta-llama/Llama-3.1-70B-Instruct` |
| Hyperbolic | `meta-llama/Llama-3.1-70B-Instruct` |
| Mira | `llama3.1-70b` |
| Mistral | `mistral-large-latest` |
| Moonshot | `moonshot-v1-32k` |
| Ollama | `llama3` |
| OpenAI | `gpt-4o` |
| OpenRouter | `anthropic/claude-sonnet-4.5` |
| Perplexity | `sonar-pro` |
| Together | `meta-llama/Llama-3.3-70B-Instruct-Turbo` |
| xAI (Grok) | `grok-3` |
| OpenAI-compatible | `gpt-4o` |

> **Tip:** Code review benefits from strong reasoning capabilities. Models like Claude Sonnet, GPT-4o, and Gemini 2.5 Flash tend to produce the most precise findings. Smaller or faster models work fine for quick feedback but may miss subtle issues.

## Ollama (Local Models)

Ollama runs locally and does not require an API key:

```bash
export NITPIK_PROVIDER=ollama
export NITPIK_MODEL=llama3
```

By default nitpik connects to `http://localhost:11434`. To use a different host, set `NITPIK_BASE_URL`:

```bash
export NITPIK_BASE_URL=http://192.168.1.100:11434
```

## Azure OpenAI

Azure requires your deployment endpoint as `NITPIK_BASE_URL` and the model is your deployment name:

```bash
export NITPIK_PROVIDER=azure
export NITPIK_BASE_URL=https://your-resource.openai.azure.com
export AZURE_OPENAI_API_KEY=your-key
export NITPIK_MODEL=your-deployment-name
```

## OpenAI-Compatible Endpoints

Use any API that speaks the OpenAI chat completions protocol — self-hosted models, corporate proxies, or alternative providers:

```bash
export NITPIK_PROVIDER=openai-compatible
export NITPIK_BASE_URL=https://your-endpoint.example.com/v1
export OPENAI_API_KEY=your-key
export NITPIK_MODEL=your-model-name
```

This works with LM Studio, vLLM, and similar services.

## Per-Profile Model Overrides

Individual reviewer profiles can specify their own model, overriding the global setting. This lets you use a cheaper model for simple checks and a more capable one for security analysis:

```markdown
---
name: security
description: Deep security analysis
model: claude-sonnet-4-5-20250929
---
```

See [Custom Profiles](07-Custom-Profiles) for the full profile format.

## Per-Task Model Overrides

A review run makes several kinds of LLM call, and the cheaper, non-review ones don't need a top-tier model. You can point them at a smaller/faster model — on the **same provider and API key** — while the per-file review keeps the strong one:

```toml
[provider]
name = "anthropic"
model = "claude-opus-4-8"               # per-file review

[provider.models]
triage  = "claude-haiku-4-5-20251001"   # auto profile selection + threat triage
summary = "claude-haiku-4-5-20251001"   # rolling PR summary (--pr-summary)
```

Or via environment variables:

```bash
export NITPIK_TRIAGE_MODEL=claude-haiku-4-5-20251001
export NITPIK_SUMMARY_MODEL=claude-haiku-4-5-20251001
```

Each override falls back to `[provider] model` (or `NITPIK_MODEL`) when unset, so omitting them keeps the previous single-model behavior. Only `triage` and `summary` are configurable: the per-file review and the critic/verify pass always use the primary model — the critic is judgment-heavy and intentionally stays on it. Token usage is attributed per model, so the run summary shows what each model cost.

See [Configuration](16-Configuration) for the full reference.

## Config File Setup

Instead of environment variables, configure the provider in `.nitpik.toml`:

```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-5-20250929"
# api_key = "..."   # possible but not recommended — use env vars for secrets
```

Or in your global config at `~/.config/nitpik/config.toml` to set a default for all repositories.

See [Configuration](16-Configuration) for the full layering order.

## Prompt Caching

For multi-file reviews, nitpik structures the prompt so that the system
prefix (agent system prompt + project documentation + commit history)
is byte-identical across every file in the run. Providers with prompt
caching can reuse that prefix on the second task and beyond, cutting
the input-token bill substantially on big diffs.

| Provider | Caching | How it activates |
|---|---|---|
| Anthropic | yes (ephemeral, 5 min) | rig-core inserts `cache_control` on large system blocks |
| OpenAI | yes (automatic) | provider caches prompts ≥ 1024 input tokens |
| Gemini | yes (implicit) | context cache kicks in above provider thresholds |
| Azure OpenAI | yes (automatic) | inherits OpenAI's behavior |
| GitHub Models | unknown | upstream caching behavior not documented |
| Cohere, DeepSeek, Groq, Mistral, others | none / provider-controlled | no client opt-in |

When cache hits occur, the run summary surfaces them:

```
  ▸ Tokens: 42.1K↑ in, 1.2K↓ out (28.0K cached, 67% hit)
```

The hit ratio (`cached_input / input`) tells you what fraction of input
tokens were served from cache. Providers without caching simply omit
the cache section from the summary.

## Related Pages

- [GitHub Models](04-GitHub-Models) — free GitHub-hosted inference with `GITHUB_TOKEN`, ideal for CI
- [Quick Start](02-Quick-Start) — run your first review
- [Configuration](16-Configuration) — all config options
- [Custom Profiles](07-Custom-Profiles) — per-profile model overrides
