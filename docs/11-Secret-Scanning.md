# Secret Scanning

nitpik detects and redacts secrets in your code **before** anything is sent to the LLM. When enabled, API keys, tokens, passwords, and other sensitive values are replaced with `[REDACTED]` in both the diff and file content.

---

## Enabling Secret Scanning

```bash
nitpik review --diff-base main --scan-secrets
```

Or enable it permanently in `.nitpik.toml`:

```toml
[secrets]
enabled = true
```

> **Tip:** Always enable `--scan-secrets` in CI pipelines. Locally, enable it when reviewing code that may contain credentials.

## What Gets Detected

nitpik ships with **200+ gitleaks-compatible rules** covering:

- Cloud provider keys (AWS, GCP, Azure)
- API tokens (GitHub, GitLab, Slack, Stripe, Twilio, etc.)
- Database connection strings
- Private keys (RSA, SSH, PGP)
- JWT tokens and bearer tokens
- Generic passwords and secrets in config files
- High-entropy strings (via Shannon entropy checks)

Secrets are detected in both the diff hunks and the full file content included in the prompt. Redaction happens before the LLM call — the provider never sees the secret values.

## Custom Rules

Add your own gitleaks-format rules:

```bash
nitpik review --diff-base main --scan-secrets --secrets-rules ./custom-rules.toml
```

Custom rules are loaded in addition to the built-in rules. The format follows the [gitleaks rule specification](https://github.com/gitleaks/gitleaks#configuration):

```toml
[[rules]]
id = "internal-api-key"
description = "Internal API key pattern"
regex = '''INTERNAL_KEY_[A-Za-z0-9]{32}'''
```

## Performance

Compiling the 200+ built-in regex rules adds roughly **20–30 seconds** of startup time on the first invocation. This cost is:

- Paid **once per run**, not per file
- Only incurred when `--scan-secrets` is enabled
- Unaffected by the number of files in the review

Normal reviews without `--scan-secrets` have no extra startup cost.

## Related Pages

- [How Reviews Work](09-How-Reviews-Work) — where secret scanning fits in the pipeline
- [Configuration](13-Configuration) — secrets config section
- [CI/CD Integration](14-CI-Integration) — enabling secret scanning in pipelines
