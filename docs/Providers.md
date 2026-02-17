# LLM Providers

nitpik is bring-your-own-model. You choose the LLM provider and supply your own API key — nitpik never proxies, stores, or meters your API calls.

---

## Supported Providers

| Provider | `NITPIK_PROVIDER` value | Provider-specific env var |
|---|---|---|
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` |
| OpenAI | `openai` | `OPENAI_API_KEY` |
| Google Gemini | `gemini` | `GEMINI_API_KEY` |
| Cohere | `cohere` | `COHERE_API_KEY` |
| DeepSeek | `deepseek` | `DEEPSEEK_API_KEY` |
| xAI (Grok) | `xai` | `XAI_API_KEY` |
| Groq | `groq` | `GROQ_API_KEY` |
| Perplexity | `perplexity` | `PERPLEXITY_API_KEY` |
| OpenAI-compatible | `openai-compatible` | `OPENAI_API_KEY` |

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

By default, nitpik uses a sensible model for each provider. Override it with `NITPIK_MODEL`:

```bash
export NITPIK_MODEL=claude-sonnet-4-20250514
```

Or in your `.nitpik.toml`:

```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-20250514"
```

> **Tip:** Code review benefits from strong reasoning capabilities. Models like Claude Sonnet, GPT-4o, and Gemini 2.5 Flash tend to produce the most precise findings. Smaller or faster models work fine for quick feedback but may miss subtle issues.

## OpenAI-Compatible Endpoints

Use any API that speaks the OpenAI chat completions protocol — self-hosted models, corporate proxies, or alternative providers:

```bash
export NITPIK_PROVIDER=openai-compatible
export NITPIK_BASE_URL=https://your-endpoint.example.com/v1
export OPENAI_API_KEY=your-key
export NITPIK_MODEL=your-model-name
```

This works with LM Studio, Ollama (with OpenAI compatibility), Azure OpenAI, and similar services.

## Per-Profile Model Overrides

Individual reviewer profiles can specify their own model, overriding the global setting. This lets you use a cheaper model for simple checks and a more capable one for security analysis:

```markdown
---
name: security
description: Deep security analysis
model: claude-sonnet-4-20250514
---
```

See [Custom Profiles](Custom-Profiles) for the full profile format.

## Config File Setup

Instead of environment variables, configure the provider in `.nitpik.toml`:

```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-20250514"
# api_key = "..."   # possible but not recommended — use env vars for secrets
```

Or in your global config at `~/.config/nitpik/config.toml` to set a default for all repositories.

See [Configuration](Configuration) for the full layering order.

## Related Pages

- [Quick Start](Quick-Start) — run your first review
- [Configuration](Configuration) — all config options
- [Custom Profiles](Custom-Profiles) — per-profile model overrides
