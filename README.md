# nitpik

**Free for personal and open-source use.** No license key needed — just install and go.

AI-powered code reviews for your team. Bring your own model, bring your own API key. One flat platform fee — no per-seat charges, no usage caps.

[Website](https://nitpik.dev) · [Documentation](https://github.com/nsrosenqvist/nitpik/wiki) · [Get a License](https://nitpik.dev) · `nitpik help`

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
export NITPIK_PROVIDER=anthropic        # or openai, gemini, cohere, deepseek, xai, groq, mistral, ollama, and more
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

Auto-select profiles based on what changed — nitpik looks at file extensions, directory structure, and project root files (like `package.json`) to pick the right combination of `frontend`, `backend`, `architect`, and `security`:

```bash
nitpik review --diff-base main --profile auto
```

Select profiles by tag — all profiles (built-in and custom) whose tags match are included:

```bash
nitpik review --diff-base main --tag security          # all profiles tagged "security"
nitpik review --diff-base main --tag css,accessibility  # union of both tags
```

Combine `--tag` with `--profile` to add tag-matched profiles on top of explicit ones:

```bash
nitpik review --diff-base main --profile backend --tag css
```

Tag matching is case-insensitive. See [Custom Agent Profiles](#custom-agent-profiles) for how to set tags on your own profiles.

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
| Bitbucket Code Insights | `--format bitbucket` | Bitbucket Pipelines (requires `BITBUCKET_TOKEN`) |
| Forgejo/Gitea PR review | `--format forgejo` | Woodpecker CI / Forgejo / Gitea (requires `FORGEJO_TOKEN`) |

nitpik exits non-zero on `error`-severity findings by default — just like standard test runners and linters. Adjust the threshold or disable it:

```bash
nitpik review --diff-base main --format github --fail-on warning  # also fail on warnings
nitpik review --diff-base main --no-fail                          # always exit 0
```

Run `nitpik help review` for the full list of flags.

### Intelligent Review Orchestration

nitpik doesn't just pass your diff to an LLM and hope for the best. Every review is carefully assembled to maximize precision and minimize noise:

- **Full-context awareness** — the LLM sees the complete file (or smart excerpts for large files), your project's conventions, commit history, and the focused diff — so it understands what changed and why it matters.
- **Multi-agent coordination** — when multiple reviewer profiles run in parallel, each one knows what the others cover and stays in its lane, eliminating duplicate findings and ensuring nothing falls through the cracks.
- **Iterative continuity** — when you push new changes, nitpik carries forward context from previous reviews so the LLM can distinguish resolved issues from persistent ones, keeping feedback consistent as your code evolves.
- **Quality post-processing** — findings are deduplicated across agents, filtered to only the lines you actually changed, and severity-normalized before they reach you. The result: actionable findings, not LLM noise.

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
| `BITBUCKET_TOKEN` | Bitbucket access token (required for `--format bitbucket`) |
| `FORGEJO_TOKEN` | Forgejo/Gitea API token (required for `--format forgejo`) |

nitpik tries to use the provider specific environment variable if it exists, and falls back to `NITPIK_API_KEY`.

---

## Custom Agent Profiles

Create a Markdown file with YAML frontmatter to define your own reviewer. See the [built-in profiles](src/agents/builtin/) for real examples of how to structure effective review prompts.

```markdown
---
name: team-conventions
description: Enforces our internal coding standards
model: claude-sonnet-4-20250514            # optional model override
tags: [style, conventions]
agentic_instructions: >                    # optional — only used in --agent mode
  Use `search_text` to find other usages of renamed symbols and verify
  they follow the new naming convention.
---

You are a code reviewer enforcing our team's conventions.

Check for:
- snake_case functions, PascalCase types
- Result-based error handling (no unwrap in production)
- Doc comments on all public items
```

The `tags` field lets you select this profile with `--tag` instead of (or alongside) `--profile`:

```bash
# Select every profile tagged "style" — including team-conventions above
nitpik review --diff-base main --profile-dir ./agents --tag style
```

Multiple profiles can share tags. For example, if two custom profiles are both tagged `css`, running `--tag css` selects them both (along with any built-in profiles that also carry the tag, such as `frontend`).

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

### Environment Passthrough

Custom command tools run in a sandboxed subprocess that uses an **allowlist** model: only essential system variables (`PATH`, `HOME`, `SHELL`, `TERM`, `LANG`, `USER`, locale prefixes like `LC_*`, and XDG directories like `XDG_*`) are inherited. Everything else — API keys, tokens, database credentials, CI secrets — is stripped by default. This prevents accidental credential leakage to LLM-invoked commands.

If your custom tools need additional environment variables (e.g. to authenticate against Jira, Docker, or AWS), declare them in the profile's `environment` frontmatter field:

```markdown
---
name: infra-reviewer
description: Reviews infrastructure changes
environment:
  - JIRA_TOKEN
  - AWS_*          # prefix glob — matches AWS_REGION, AWS_SECRET_ACCESS_KEY, etc.
  - DOCKER_HOST
tools:
  - name: deploy_check
    description: Check deployment status
    command: curl -sH "Authorization: Bearer $JIRA_TOKEN" https://jira.example.com/status
---

You are an infrastructure reviewer...
```

Exact names and prefix globs (ending with `*`) are supported. All env vars not in the default safe set are stripped unless explicitly listed in `environment`.

---

## CI / CD Integration

> **Note:** As of the initial release, only GitHub Actions has been thoroughly tested. More in-depth testing of other CI platforms will follow.

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
      - uses: actions/cache@v4
        with:
          path: ~/.config/nitpik/cache
          key: nitpik-${{ github.repository }}
          save-always: true
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

> **Security:** Always pass API keys via `${{ secrets.* }}` — never hardcode them in workflow files.

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
      - uses: actions/cache@v4
        with:
          path: ~/.config/nitpik/cache
          key: nitpik-${{ github.repository }}
          save-always: true
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
  image: ghcr.io/nsrosenqvist/nitpik:latest
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
  cache:
    key: nitpik
    paths:
      - .nitpik-cache/
    when: always
  artifacts:
    reports:
      codequality: gl-code-quality-report.json
  variables:
    NITPIK_PROVIDER: anthropic
    ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY
    NITPIK_LICENSE_KEY: $NITPIK_LICENSE_KEY
    XDG_CONFIG_HOME: $CI_PROJECT_DIR/.nitpik-cache
```

Findings appear in the merge request Code Quality widget.

### Bitbucket Pipelines

The `bitbucket` format posts findings as Code Insights annotations via the Bitbucket API. This requires a **Repository Access Token** (or App Password) with the `pullrequest` and `repository:write` scopes.

Create one under **Repository settings → Access tokens** and add it as a [pipeline variable](https://support.atlassian.com/bitbucket-cloud/docs/variables-and-secrets/) named `BITBUCKET_TOKEN`.

```yaml
definitions:
  caches:
    nitpik: /root/.config/nitpik/cache

pipelines:
  pull-requests:
    '**':
      - step:
          image: ghcr.io/nsrosenqvist/nitpik:latest
          caches:
            - nitpik
          script:
            - git fetch origin "$BITBUCKET_PR_DESTINATION_BRANCH"
            - nitpik review
                --diff-base "origin/$BITBUCKET_PR_DESTINATION_BRANCH"
                --profile security,backend
                --format bitbucket
                --fail-on error
                --scan-secrets
          variables:
            NITPIK_PROVIDER: anthropic
            ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY
            NITPIK_LICENSE_KEY: $NITPIK_LICENSE_KEY
            BITBUCKET_TOKEN: $BITBUCKET_TOKEN
```

> **Security:** Add `ANTHROPIC_API_KEY`, `NITPIK_LICENSE_KEY`, and `BITBUCKET_TOKEN` as **secured** pipeline variables — never hardcode them in `bitbucket-pipelines.yml`.

### Woodpecker CI (Forgejo / Gitea / Codeberg)

The `forgejo` format posts findings as inline PR review comments via the Forgejo/Gitea API. This requires a **personal access token** with (at minimum) the **`write:repository`** scope.

Create one under **User settings → Applications → Generate New Token** in your Forgejo or Gitea instance. Add it as a Woodpecker secret named `forgejo_token` so it is exposed as `FORGEJO_TOKEN` at runtime.

```yaml
when:
  event: pull_request

steps:
  - name: ai-review
    image: ghcr.io/nsrosenqvist/nitpik:latest
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
    volumes:
      - nitpik-cache:/root/.config/nitpik/cache
```

> **Security:** Add `anthropic_api_key`, `nitpik_license_key`, and `forgejo_token` as [Woodpecker secrets](https://woodpecker-ci.org/docs/usage/secrets) — never hardcode them in the pipeline file.

---

## Secret Scanning

nitpik ships with 200+ gitleaks-compatible rules and Shannon entropy checks. When enabled, secrets are detected and redacted **before** any code is sent to the LLM.

```bash
nitpik review --diff-base main --scan-secrets
```

> **Performance note:** Compiling the built-in regex rules adds roughly 20–30 seconds of startup time. This cost is paid once per invocation and only when secret scanning is enabled — normal reviews without `--scan-secrets` are unaffected.

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

## Project Documentation Context

nitpik automatically detects documentation files in your repository root and includes them in the review prompt so the LLM understands your team's conventions.

### Priority review context (`REVIEW.md` / `NITPIK.md`)

If a **`REVIEW.md`** or **`NITPIK.md`** file exists in the repo root, nitpik uses *only* those files as review context and skips the generic doc list entirely. This lets you provide focused review guidance without polluting the prompt with coding-agent instructions (like `AGENTS.md` or `.cursorrules`) that aren't relevant to a reviewer.

Both files can coexist — if both are present, both are included.

### Generic fallback docs

When no priority file is found, nitpik falls back to scanning for these well-known files:

`AGENTS.md`, `ARCHITECTURE.md`, `CONVENTIONS.md`, `CONTRIBUTING.md`, `CLAUDE.md`, `.github/copilot-instructions.md`, `.cursorrules`, `CODING_GUIDELINES.md`, `STYLE_GUIDE.md`, `DEVELOPMENT.md`

Files larger than 256 KB are skipped.

### Controlling project docs

| Flag | Default | Effect |
|---|---|---|
| `--no-project-docs` | `false` | Skip all auto-detected project documentation files |
| `--exclude-doc <NAME>` | *(none)* | Exclude specific docs by filename (comma-separated) |

Skip all project docs (useful to reduce prompt size or cost):

```bash
nitpik review --diff-base main --no-project-docs
```

Exclude only specific files while keeping the rest:

```bash
nitpik review --diff-base main --exclude-doc AGENTS.md
nitpik review --diff-base main --exclude-doc AGENTS.md,CONTRIBUTING.md
```

> **Note:** Excluding all priority files (e.g. `--exclude-doc REVIEW.md`) causes nitpik to fall back to the generic doc list.

---

## Commit History Context

When reviewing a git ref diff (`--diff-base`), nitpik includes the commit log between the base and HEAD in the review prompt. This gives the LLM insight into the *intent* behind the changes — commit messages explain *why* code was changed, helping the reviewer calibrate severity and avoid flagging deliberate refactors or fixes.

Up to 50 commit summaries (newest first) are included automatically. This feature only applies to `--diff-base` mode — stdin diffs, diff files, and directory scans have no commit history to include.

| Flag | Default | Effect |
|---|---|---|
| `--no-commit-context` | `false` | Skip injecting commit summaries into the review prompt |

Disable commit context when commit messages are noisy or to reduce token usage:

```bash
nitpik review --diff-base main --no-commit-context
```

---

## Caching

nitpik caches review results by content hash. Unchanged files are not re-reviewed, saving time and API cost.

When a file changes and the cache is invalidated, nitpik automatically includes the **previous findings** in the LLM prompt. This lets the model distinguish resolved issues from persistent or new ones, improving consistency across successive reviews of the same file.

### Branch-scoped prior findings

Prior findings are tracked per **branch** so that parallel PRs reviewing the same file don't cross-contaminate each other. nitpik detects the branch from `git rev-parse --abbrev-ref HEAD`, falling back to CI environment variables (`GITHUB_HEAD_REF`, `CI_COMMIT_BRANCH`, `CI_MERGE_REQUEST_SOURCE_BRANCH_NAME`, `BITBUCKET_BRANCH`, `CI_BRANCH`) when running in detached-HEAD mode.

Previous findings are sorted by severity (errors first) before being injected, so the most important context is always preserved.

### Stale sidecar cleanup

Sidecar metadata files older than **30 days** are automatically removed at the start of each review run, keeping the cache directory from growing unbounded after branches are merged or deleted.

```bash
nitpik cache stats   # show entry count and size
nitpik cache clear   # wipe the cache (including all sidecar metadata)
nitpik cache path    # print the cache directory
```

| Flag | Default | Effect |
|---|---|---|
| `--no-cache` | `false` | Disable caching entirely for this run |
| `--no-prior-context` | `false` | Skip injecting previous findings on cache invalidation |
| `--max-prior-findings <N>` | unlimited | Cap the number of prior findings included in the prompt |

Disable caching for a single run with `--no-cache`. Use `--no-prior-context` for a clean-slate review without prior-finding context, or `--max-prior-findings` to limit prompt token usage when prior reviews produced many findings.

---

## Limitations & Disclaimer

nitpik uses third-party large language models (LLMs) to analyze code. **All findings are AI-generated and advisory.** They may be incorrect, incomplete, or hallucinated. nitpik does not guarantee code quality, security, or correctness.

- **Always review AI suggestions with human judgment** before acting on them.
- **Code diffs are sent to your configured LLM provider** (e.g. Anthropic, OpenAI, Gemini). nitpik does not store or retain your code, but the LLM provider's data policies apply. Choose a provider whose terms you trust.
- **Enable `--scan-secrets`** to detect and redact secrets before code is sent to the LLM. Without this flag, secrets present in your diffs will be transmitted to the provider.
- **Provider integrations rely on a third-party open-source library.** LLM provider support may change, break, or be removed due to upstream updates outside of nitpik's control. If you plan to purchase a commercial license, **please verify that your provider and model work correctly using the free unlicensed version first.** No license key is required for this — just install and test with your own API key.
- nitpik is a development aid, not a replacement for human code review, testing, or security auditing.

---

## Telemetry

nitpik sends a single anonymous heartbeat per review run — **no code, file names, findings, or PII**. Just aggregate counts (files, lines, profiles) and whether you're in CI. You can verify exactly what is sent by reading [`src/telemetry/mod.rs`](src/telemetry/mod.rs).

The heartbeat payload contains only:

- A random run ID (not persisted across runs)
- File count and total changed lines
- Number of agent profiles used
- Whether a commercial license is active
- Whether the run is in a CI environment
- CLI version string

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

## Version Information

Print detailed build metadata:

```bash
nitpik version
```

Outputs version, git commit, build date, and target triple:

```
nitpik 0.2.0
commit:     a1b2c3d
built:      2026-02-14
target:     x86_64-unknown-linux-gnu
```

For the short one-liner, use the standard `--version` flag:

```bash
nitpik --version         # nitpik 0.2.0
```

---

## Further Help

Every command and flag is documented in the built-in help:

```bash
nitpik help              # overview of all commands
nitpik help review       # full review flag reference
nitpik help license      # license management
nitpik help cache        # cache management
nitpik help update       # self-update
nitpik version           # version, commit, build date, target
```

---

## Feedback & Issues

Found a bug, false positive, or harmful AI output? Please [open an issue](https://github.com/nsrosenqvist/nitpik/issues) on GitHub. Feedback helps improve nitpik for everyone.

---

## License

Licensed under the [Business Source License 1.1](LICENSE).

**Free for personal, educational, and open-source use — no license key required.** Commercial use requires a license. One flat fee, any team size, unlimited usage. Visit [nitpik.dev](https://nitpik.dev) for pricing.

You bring your own LLM provider and API key — nitpik never proxies, stores, or meters your API calls.

The code converts to [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0) three years after each release.
