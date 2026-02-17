# Quick Start

Run your first AI code review in three steps.

---

## 1. Install nitpik

Download the latest binary for your platform:

```bash
# Linux (x86_64)
curl -sSfL https://github.com/nsrosenqvist/nitpik/releases/latest/download/nitpik-x86_64-unknown-linux-gnu.tar.gz | sudo tar xz -C /usr/local/bin
```

See [Installation](01-Installation) for macOS, Docker, and other options.

## 2. Connect an LLM Provider

Set two environment variables — a provider name and the corresponding API key:

```bash
export NITPIK_PROVIDER=anthropic
export ANTHROPIC_API_KEY=sk-ant-...
```

nitpik supports Anthropic, OpenAI, Gemini, Cohere, DeepSeek, xAI, Groq, Perplexity, and any OpenAI-compatible endpoint. See [LLM Providers](03-Providers) for the full list.

## 3. Run a Review

From your repository, diff against a branch and review:

```bash
nitpik review --diff-base main
```

nitpik diffs your current branch against `main`, picks a reviewer profile, and prints findings:

```
nitpik · Free for personal & open-source use. Commercial use requires a license.

✔ w/handler.rs done

 ✖ error in handler.rs:21
   Backend crashes due to unhandled file I/O and parsing errors — The
   `load_users` function uses `unwrap()` for file reading and parsing,
   and accesses array elements without bounds checking.
   → Implement robust error handling (e.g., using `Result` and propagating
     errors) instead of `unwrap()`. Add bounds checking for array access.

 ⚠ warning in handler.rs:36
   N+1 query in `get_users_by_ids` — Calling `get_user` in a loop for
   each ID results in an N+1 query pattern, leading to significant
   performance degradation for large ID lists.
   → Consider implementing a batch fetch mechanism that retrieves all
     users in a single operation.

───────────────────────────────────
 2 findings: 1 errors, 1 warnings, 0 infos
```

Each finding includes:

- **Severity** — `error` (confirmed bug), `warning` (likely problem), or `info` (suggestion)
- **Location** — file and line number
- **Title** — one-line summary
- **Message** — detailed explanation
- **Suggestion** — recommended fix

## What's Next?

- **Run multiple reviewers** — add `--profile backend,security` to get specialist perspectives. See [Reviewer Profiles](05-Reviewer-Profiles).
- **Set up CI** — output findings as GitHub annotations, GitLab Code Quality, or Bitbucket Code Insights. See [CI/CD Integration](14-CI-Integration).
- **Enable secret scanning** — add `--scan-secrets` to detect and redact secrets before they reach the LLM. See [Secret Scanning](11-Secret-Scanning).
- **Explore agentic mode** — add `--agent` to let the LLM read files and search your codebase for deeper analysis. See [Agentic Mode](07-Agentic-Mode).
- **Create team config** — drop a `.nitpik.toml` in your repo root. See [Configuration](13-Configuration).

## Related Pages

- [Installation](01-Installation) — all install methods
- [LLM Providers](03-Providers) — provider setup details
- [Diff Inputs](04-Diff-Inputs) — all the ways to feed code to nitpik
