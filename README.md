# nitpik

**Free for personal and open-source use.** No license key needed — just install and go.

AI-powered code reviews for your team. Bring your own model, bring your own API key. One flat platform fee — no per-seat charges, no usage caps.

[Website](https://nitpik.dev) · [Get a License](https://nitpik.dev) · `nitpik help`

---

## Why nitpik?

- **Bring your own model** — use Anthropic, OpenAI, Gemini, Cohere, DeepSeek, xAI, Groq, Perplexity, or any OpenAI-compatible API. Your keys, your infrastructure, your data.
- **Flat platform fee** — one price for your whole team. No per-seat licensing, no usage-based billing, no surprises.
- **Free for personal & OSS** — use nitpik on personal projects and open-source repos at no cost, forever. No license key required.
- **Single binary + Docker image** — drop it into any CI pipeline in minutes.
- **Configurable reviewer agents** — built-in profiles or custom Markdown-defined reviewers with your team's conventions.
- **Agentic mode** — let the LLM explore your codebase with built-in and custom tools.
- **Secret scanning** — 200+ rules detect and redact secrets before code reaches the LLM.
- **Every major CI platform** — GitHub Actions, GitLab CI, Bitbucket Pipelines, Woodpecker/Forgejo/Gitea.

---

## Getting Started

### 1. Install

**Pre-built binary (recommended)**

Download the latest binary for your platform from the [GitHub Releases page](https://github.com/nsrosenqvist/nitpik/releases/latest) and place it on your `PATH`.

Or build from source:

```bash
cargo install --path .
```

Once installed, update to the latest release at any time:

```bash
nitpik update
```

**Docker**

```bash
docker pull ghcr.io/nsrosenqvist/nitpik:latest
```

### 2. Activate Your License (commercial use only)

For commercial use, activate your license key:

```bash
nitpik license activate <YOUR_LICENSE_KEY>
nitpik license status   # verify activation
```

The key is stored in `~/.config/nitpik/config.toml`. You can also set the `NITPIK_LICENSE_KEY` environment variable in CI.

**Personal and open-source projects do not need a license key.**

### 3. Connect an LLM Provider

nitpik is bring-your-own-model. Set two environment variables — a provider name and the corresponding API key:

```bash
export NITPIK_PROVIDER=anthropic        # or openai, gemini, cohere, deepseek, xai, groq, perplexity
export ANTHROPIC_API_KEY=sk-...         # provider-specific key
```

Or use `NITPIK_API_KEY` as a universal fallback. To use a custom or self-hosted endpoint (any OpenAI-compatible API), also set `NITPIK_BASE_URL`.

### 4. Run Your First Review

```bash
nitpik review --diff-base main
```

That's it. nitpik diffs your current branch against `main`, picks a reviewer profile, scans for secrets, and prints findings to your terminal:

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

---

## Core Concepts

### Diff Inputs

nitpik needs a diff to review. Pick whichever suits your workflow:

```bash
nitpik review --diff-base main              # git diff against a branch/commit
nitpik review --scan src/main.rs            # review a file directly (no git)
nitpik review --diff-file changes.patch     # pre-computed unified diff
git diff main | nitpik review --diff-stdin  # piped from another tool
```

### Reviewer Profiles

Profiles are specialist reviewers with their own system prompts and focus areas. Four ship built-in:

| Profile | Focus |
|---|---|
| `backend` | Correctness, error handling, performance, API design |
| `frontend` | Accessibility, rendering, state management, UX |
| `architect` | System design, coupling, abstractions, scalability |
| `security` | Vulnerabilities, injection, auth, data exposure |

Use one or combine several:

```bash
nitpik review --diff-base main --profile backend,security
```

Auto-select profiles based on the files in the diff:

```bash
nitpik review --diff-base main --profile auto
```

List all available profiles (including custom ones):

```bash
nitpik profiles
nitpik profiles --profile-dir ./agents
```

### Output Formats

| Format | Flag | Use case |
|---|---|---|
| Styled terminal | `--format terminal` | Local development (default) |
| JSON | `--format json` | Custom tooling / dashboards |
| GitHub annotations | `--format github` | GitHub Actions |
| GitLab Code Quality | `--format gitlab` | GitLab CI merge request widgets |
| Bitbucket Code Insights | `--format bitbucket` | Bitbucket Pipelines |
| Forgejo/Gitea PR review | `--format forgejo` | Woodpecker CI / Forgejo / Gitea |

Fail a CI build on findings above a threshold:

```bash
nitpik review --diff-base main --format github --fail-on warning
```

Run `nitpik help review` for the full list of flags.

---

## Configuration

Settings are layered — each level overrides the one below it:

**CLI flags → environment variables → `.nitpik.toml` (repo) → `~/.config/nitpik/config.toml` (global) → defaults**

### Project Config (`.nitpik.toml`)

Drop this in your repo root to set defaults for everyone on the team:

```toml
[provider]
name = "anthropic"
model = "claude-sonnet-4-20250514"

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

[secrets]
enabled = false
```

### Environment Variables

| Variable | Purpose |
|---|---|
| `NITPIK_PROVIDER` | LLM provider (`anthropic`, `openai`, `gemini`, etc.) |
| `NITPIK_MODEL` | Model name (e.g. `claude-sonnet-4-20250514`) |
| `NITPIK_API_KEY` | Universal API key fallback |
| `NITPIK_BASE_URL` | Custom endpoint for OpenAI-compatible APIs |
| `NITPIK_LICENSE_KEY` | License key (alternative to `nitpik license activate`) |
| `ANTHROPIC_API_KEY` | Anthropic-specific key |
| `OPENAI_API_KEY` | OpenAI-specific key (also used for openai-compatible) |
| `GEMINI_API_KEY` | Gemini-specific key |

Nitpik tries to use the provider specific environment variable if it exists, and falls back to `NITPIK_API_KEY`.

---

## Custom Agent Profiles

Create a Markdown file with YAML frontmatter to define your own reviewer:

```markdown
---
name: team-conventions
description: Enforces our internal coding standards
model: claude-sonnet-4-20250514            # optional model override
tags: [style, conventions]
---

You are a code reviewer enforcing our team's conventions.

Check for:
- snake_case functions, PascalCase types
- Result-based error handling (no unwrap in production)
- Doc comments on all public items
```

Use it directly by path, or place it in a directory and reference by name:

```bash
nitpik review --diff-base main --profile ./team-conventions.md
nitpik review --diff-base main --profile-dir ./agents --profile team-conventions
```

Validate a profile before using it:

```bash
nitpik validate ./team-conventions.md
```

### Custom Agentic Tools

Profiles can define CLI tools that the LLM can invoke during agentic reviews. Add a `tools` section to the frontmatter:

```markdown
---
name: test-aware-reviewer
description: Reviews code and can run the test suite
tools:
  - name: run_tests
    description: Run the test suite with an optional filter
    command: cargo test
    parameters:
      - name: filter
        type: string
        description: Test name filter
        required: false
---

You are a reviewer that validates changes against the test suite...
```

When `--agent` is enabled, the LLM can call `run_tests` (along with the built-in `read_file`, `search_text`, and `list_directory` tools) to explore the codebase and gather context.

```bash
nitpik review --diff-base main --profile ./test-aware-reviewer.md --agent
```

---

## CI / CD Integration

### Docker

The official image ships with `git` and the `nitpik` binary. Mount your repo and pass environment variables:

```bash
docker run --rm \
  -v "$(pwd)":/repo \
  -e NITPIK_PROVIDER=anthropic \
  -e ANTHROPIC_API_KEY \
  -e NITPIK_LICENSE_KEY \
  ghcr.io/nsrosenqvist/nitpik:latest review --diff-base main --scan-secrets
```

### GitHub Actions

The easiest way — use the official action:

```yaml
on: pull_request

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: nsrosenqvist/nitpik@v1
        with:
          profiles: backend,security
          fail_on: warning
          scan_secrets: "true"
        env:
          NITPIK_PROVIDER: anthropic
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          NITPIK_LICENSE_KEY: ${{ secrets.NITPIK_LICENSE_KEY }}
```

The action auto-detects the PR target branch, downloads the binary, and outputs findings as inline annotations.

<details>
<summary>Manual setup (without the action)</summary>

```yaml
on: pull_request

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - name: Install nitpik
        run: curl -sSfL https://github.com/nsrosenqvist/nitpik/releases/latest/download/nitpik-x86_64-unknown-linux-gnu.tar.gz | sudo tar xz -C /usr/local/bin
      - name: AI Code Review
        run: |
          nitpik review \
            --diff-base "origin/$GITHUB_BASE_REF" \
            --profile backend,security \
            --format github \
            --fail-on warning \
            --scan-secrets
        env:
          NITPIK_PROVIDER: anthropic
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
          NITPIK_LICENSE_KEY: ${{ secrets.NITPIK_LICENSE_KEY }}
```

</details>

Findings appear as inline annotations on the pull request.

### GitLab CI/CD

```yaml
code-review:
  stage: test
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
  script:
    - git fetch origin "$CI_MERGE_REQUEST_TARGET_BRANCH_NAME"
    - nitpik review
        --diff-base "origin/$CI_MERGE_REQUEST_TARGET_BRANCH_NAME"
        --profile backend,security
        --format gitlab
        --fail-on warning
        --scan-secrets
        > gl-code-quality-report.json
  artifacts:
    reports:
      codequality: gl-code-quality-report.json
  variables:
    NITPIK_PROVIDER: anthropic
    ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY
    NITPIK_LICENSE_KEY: $NITPIK_LICENSE_KEY
```

Findings appear in the merge request Code Quality widget.

### Bitbucket Pipelines

```yaml
pipelines:
  pull-requests:
    '**':
      - step:
          script:
            - git fetch origin "$BITBUCKET_PR_DESTINATION_BRANCH"
            - nitpik review
                --diff-base "origin/$BITBUCKET_PR_DESTINATION_BRANCH"
                --profile security,backend
                --format bitbucket
                --fail-on error
                --scan-secrets
```

### Woodpecker CI (Forgejo / Gitea / Codeberg)

```yaml
when:
  event: pull_request

steps:
  - name: ai-review
    image: nitpik:latest
    commands:
      - git fetch origin "$CI_COMMIT_TARGET_BRANCH"
      - nitpik review
          --diff-base "origin/$CI_COMMIT_TARGET_BRANCH"
          --profile backend,security
          --format forgejo
          --fail-on warning
          --scan-secrets
    secrets: [forgejo_token, anthropic_api_key, nitpik_license_key]
    environment:
      NITPIK_PROVIDER: anthropic
```

---

## Secret Scanning

nitpik ships with 200+ gitleaks-compatible rules and Shannon entropy checks. When enabled, secrets are detected and redacted **before** any code is sent to the LLM.

```bash
nitpik review --diff-base main --scan-secrets
```

Always-on in CI:

```toml
# .nitpik.toml
[secrets]
enabled = true
```

Bring your own rules:

```bash
nitpik review --diff-base main --scan-secrets --secrets-rules ./custom-rules.toml
```

---

## Caching

nitpik caches review results by content hash. Unchanged files are not re-reviewed, saving time and API cost.

```bash
nitpik cache stats   # show entry count and size
nitpik cache clear   # wipe the cache
nitpik cache path    # print the cache directory
```

Disable caching for a single run with `--no-cache`.

---

## Telemetry

nitpik sends a single anonymous heartbeat per review run — **no code, file names, findings, or PII**. Just aggregate counts (files, lines, profiles) and whether you're in CI.

Disable it:

```bash
nitpik review --diff-base main --no-telemetry   # per-run
export NITPIK_TELEMETRY=false                    # env var
```

```toml
# .nitpik.toml or ~/.config/nitpik/config.toml
[telemetry]
enabled = false
```

---

## Updating

nitpik can update itself to the latest release from GitHub:

```bash
nitpik update           # update to latest version (skips if already current)
nitpik update --force   # re-download even if already on latest
```

The update process downloads the release archive for your platform, verifies its SHA256 checksum, and atomically replaces the running binary.

If the binary is installed in a system directory (e.g. `/usr/local/bin`), you may need `sudo`:

```bash
sudo nitpik update
```

> **Note:** In Docker containers and CI environments, nitpik will print a warning suggesting you rebuild the image or pin a version in your pipeline instead of self-updating.

---

## Further Help

Every command and flag is documented in the built-in help:

```bash
nitpik help              # overview of all commands
nitpik help review       # full review flag reference
nitpik help license      # license management
nitpik help cache        # cache management
nitpik help update       # self-update
```

---

## License

Licensed under the [Business Source License 1.1](LICENSE).

**Free for personal, educational, and open-source use — no license key required.** Commercial use requires a license. One flat fee, any team size, unlimited usage. Visit [nitpik.dev](https://nitpik.dev) for pricing.

You bring your own LLM provider and API key — nitpik never proxies, stores, or meters your API calls.

The code converts to [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0) three years after each release.
