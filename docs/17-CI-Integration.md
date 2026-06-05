# CI/CD Integration

nitpik integrates with every major CI platform. This page covers setup for each, including token management, caching, and secret handling best practices.

> **Note:** As of the initial release, only GitHub Actions has been thoroughly tested. More in-depth testing of other CI platforms will follow.

---

## Docker

The official Docker image ships with `git` and the `nitpik` binary:

```bash
docker pull ghcr.io/nsrosenqvist/nitpik:latest
```

Mount your repository and pass environment variables:

```bash
docker run --rm \
  -v "$(pwd)":/repo \
  -e NITPIK_PROVIDER=anthropic \
  -e ANTHROPIC_API_KEY \
  -e NITPIK_LICENSE_KEY \
  ghcr.io/nsrosenqvist/nitpik:latest review --diff-base main --scan-secrets
```

## GitHub Actions

### Using the Official Action (Recommended)

```yaml
on: pull_request

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: actions/cache/restore@v4
        with:
          path: ~/.config/nitpik/cache
          key: nitpik-${{ github.repository }}
      - uses: nsrosenqvist/nitpik@v3
        with:
          profiles: security,performance
          fail_on: warning
          scan_secrets: "true"
        env:
          NITPIK_PROVIDER: ${{ vars.NITPIK_PROVIDER }}
          NITPIK_MODEL: ${{ vars.NITPIK_MODEL }}
          NITPIK_API_KEY: ${{ secrets.NITPIK_API_KEY }}
          NITPIK_LICENSE_KEY: ${{ secrets.NITPIK_LICENSE_KEY }}
      - uses: actions/cache/save@v4
        if: always()
        with:
          path: ~/.config/nitpik/cache
          key: nitpik-${{ github.repository }}
```

The action auto-detects the PR target branch, downloads the binary, and outputs findings as inline annotations on the pull request.

> **Tip:** Store `NITPIK_PROVIDER` and `NITPIK_MODEL` as **repository variables** (Settings → Secrets and variables → Actions → Variables) and `NITPIK_API_KEY` as a **repository secret**. Repository variables are not automatically available as environment variables — reference them with `${{ vars.* }}` in your workflow.

### Manual Setup

If you prefer not to use the action:

```yaml
on: pull_request

jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: actions/cache/restore@v4
        with:
          path: ~/.config/nitpik/cache
          key: nitpik-${{ github.repository }}
      - name: Install nitpik
        run: curl -sSfL https://github.com/nsrosenqvist/nitpik/releases/latest/download/nitpik-x86_64-unknown-linux-gnu.tar.gz | sudo tar xz -C /usr/local/bin
      - name: AI Code Review
        run: |
          nitpik review \
            --diff-base "origin/$GITHUB_BASE_REF" \
            --profile security,performance \
            --format github \
            --fail-on warning \
            --scan-secrets
        env:
          NITPIK_PROVIDER: ${{ vars.NITPIK_PROVIDER }}
          NITPIK_MODEL: ${{ vars.NITPIK_MODEL }}
          NITPIK_API_KEY: ${{ secrets.NITPIK_API_KEY }}
          NITPIK_LICENSE_KEY: ${{ secrets.NITPIK_LICENSE_KEY }}
      - uses: actions/cache/save@v4
        if: always()
        with:
          path: ~/.config/nitpik/cache
          key: nitpik-${{ github.repository }}
```

**Key details:**
- `fetch-depth: 0` is required for `--diff-base` to have access to the full git history.
- `--format github` outputs findings as workflow commands that appear as inline PR annotations.
- `--fail-on warning` causes the step to fail if any warning or error is found.
- `actions/cache/restore` and `actions/cache/save` are used as separate steps so the cache is always persisted — even when `--fail-on` causes the review step to exit non-zero.

> **Security:** Always pass API keys via `${{ secrets.* }}` — never hardcode them in workflow files.

### Trigger a review from a PR comment

`--format github-pr-review` posts a real PR review (inline comments + summary) rather than
annotations, and nitpik resolves the PR number from an `issue_comment` event, so you can let
contributors re-run a review on demand by commenting `@nitpik review`:

```yaml
on:
  issue_comment:
    types: [created]

permissions:
  pull-requests: write   # required to post the review

jobs:
  review:
    # Only on PR comments that ask for a review.
    if: ${{ github.event.issue.pull_request && contains(github.event.comment.body, '@nitpik review') }}
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          # issue_comment runs on the default branch — check out the PR head.
          ref: refs/pull/${{ github.event.issue.number }}/merge
      - name: Install nitpik
        run: curl -sSfL https://github.com/nsrosenqvist/nitpik/releases/latest/download/nitpik-x86_64-unknown-linux-gnu.tar.gz | sudo tar xz -C /usr/local/bin
      - name: AI Code Review
        run: |
          nitpik review \
            --diff-base "origin/${{ github.event.pull_request.base.ref || 'main' }}" \
            --profile security,performance \
            --format github-pr-review \
            --force-review
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          NITPIK_PROVIDER: ${{ vars.NITPIK_PROVIDER }}
          NITPIK_MODEL: ${{ vars.NITPIK_MODEL }}
          NITPIK_API_KEY: ${{ secrets.NITPIK_API_KEY }}
          NITPIK_LICENSE_KEY: ${{ secrets.NITPIK_LICENSE_KEY }}
```

**Key details:**
- `if: github.event.issue.pull_request` ensures the job only runs on comments on **pull requests**, not plain issues.
- `--format github-pr-review` posts inline review comments via `GITHUB_TOKEN`; it needs `permissions: pull-requests: write`.
- `--force-review` posts a fresh review even when nothing changed since the last run — without it, an explicit re-trigger on an unchanged PR would stay quiet (nitpik suppresses re-runs that would only repeat themselves). Per-finding dedup still applies, so already-posted findings won't spawn duplicate comment threads.
- Pair with `--pr-threads` (feed prior comments + replies into the review) and `--resolve-addressed` (auto-resolve threads a later push fixed) for a fully conversational re-review.

## GitLab CI/CD

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
        --profile security,performance
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

**Key details:**
- `--format gitlab` outputs a [Code Quality report](https://docs.gitlab.com/ee/ci/testing/code_quality.html) JSON file.
- Upload it as a `codequality` artifact to see findings in the merge request Code Quality widget.
- Set `XDG_CONFIG_HOME` to a path inside the project directory so the cache is preserved between runs.
- `when: always` ensures the cache is saved even when `--fail-on` causes the job to exit non-zero. Without it, GitLab only saves the cache on success.

### Inline merge-request review (`gitlab-mr-review`)

Instead of the Code Quality widget, `--format gitlab-mr-review` posts a **real inline MR review** — a summary note plus a positioned discussion on each finding's line. Like `github-pr-review`, it is dedup-aware (a re-run only posts findings not already raised) and supports `--request-changes`.

```yaml
nitpik-review:
  stage: test
  image: ghcr.io/nsrosenqvist/nitpik:latest
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
  script:
    - nitpik review
        --diff-base "$CI_MERGE_REQUEST_DIFF_BASE_SHA"
        --format gitlab-mr-review
  variables:
    NITPIK_PROVIDER: anthropic
    ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY
    NITPIK_LICENSE_KEY: $NITPIK_LICENSE_KEY
    GITLAB_TOKEN: $GITLAB_TOKEN
```

**Key details:**
- Runs in a **merge-request pipeline** — nitpik publishes when `CI_MERGE_REQUEST_IID` is set (provided automatically on MR pipelines).
- Authenticates with a `GITLAB_TOKEN` (a project/personal access token with `api` scope) or the pipeline's `CI_JOB_TOKEN`. Add it as a **masked** CI/CD variable — never hardcode it.
- Anchors comments using the MR's diff refs; comments that can't be anchored to a changed line are dropped rather than failing the run.
- Choose this over `--format gitlab` when you want conversational inline review comments rather than the Code Quality widget.

## Bitbucket Pipelines

The `bitbucket` format posts findings as Code Insights annotations via the Bitbucket API. Inside Bitbucket Pipelines, authentication is handled automatically — no token required.

### Pipeline Config

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
```

> **Note:** Inside Bitbucket Pipelines, nitpik uses the built-in authentication proxy at `localhost:29418` to post Code Insights — no `BITBUCKET_TOKEN` needed. If you run nitpik outside Pipelines (e.g. a self-hosted runner), set `BITBUCKET_TOKEN` with `pullrequest` and `repository:write` scopes.

### Alternative: Checkstyle Format

If you prefer a file-based approach without any API calls, use `--format checkstyle` and pipe the output to the [Checkstyle Code Insight Report pipe](https://bitbucket.org/product/features/pipelines/integrations?search=checkstyle):

```yaml
pipelines:
  pull-requests:
    '**':
      - step:
          image: ghcr.io/nsrosenqvist/nitpik:latest
          script:
            - git fetch origin "$BITBUCKET_PR_DESTINATION_BRANCH"
            - nitpik review
                --diff-base "origin/$BITBUCKET_PR_DESTINATION_BRANCH"
                --profile security,backend
                --format checkstyle
                --fail-on error
                --scan-secrets
                > checkstyle-report.xml
          variables:
            NITPIK_PROVIDER: anthropic
            ANTHROPIC_API_KEY: $ANTHROPIC_API_KEY
            NITPIK_LICENSE_KEY: $NITPIK_LICENSE_KEY
```

> **Security:** Add `ANTHROPIC_API_KEY` and `NITPIK_LICENSE_KEY` as **secured** pipeline variables — never hardcode them in `bitbucket-pipelines.yml`.

## Other CI Platforms

For CI platforms without a dedicated output format, use `--format checkstyle` to produce standard [Checkstyle XML](https://checkstyle.sourceforge.io/) and feed it into a tool that your platform supports:

- **Jenkins** — the [Warnings Next Generation](https://plugins.jenkins.io/warnings-ng/) plugin natively ingests checkstyle XML and displays findings in build results.
- **Any platform** — [reviewdog](https://github.com/reviewdog/reviewdog) accepts checkstyle XML via `-f=checkstyle` and posts annotations to GitHub, GitLab, Bitbucket, Gitea, and more.

```bash
# Example: pipe nitpik output through reviewdog
nitpik review --diff-base main --format checkstyle | reviewdog -f=checkstyle -reporter=github-pr-review
```

See [Output Formats — Checkstyle XML](09-Output-Formats#checkstyle-xml) for details.

## Woodpecker CI (Forgejo / Gitea / Codeberg)

The `forgejo` format posts findings as inline PR review comments via the Forgejo/Gitea API.

### Token Setup

Create a **personal access token** with at minimum the `write:repository` scope under **User settings → Applications → Generate New Token** in your Forgejo or Gitea instance.

Add it as a Woodpecker secret named `forgejo_token` so it's exposed as `FORGEJO_TOKEN` at runtime.

### Pipeline Config

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
          --profile security,performance
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

## CI Best Practices

### Caching

Always cache `~/.config/nitpik/cache` (or the Docker equivalent) between runs. This avoids re-reviewing unchanged files and reduces API cost significantly on iterative PRs.

### Secret Scanning

Enable `--scan-secrets` in CI pipelines. This catches accidentally committed secrets and redacts them before they reach the LLM.

### Fail-On Threshold

By default, nitpik exits non-zero when any finding has severity `error`. Use `--fail-on` to adjust the threshold:
- `--fail-on error` — block only on confirmed bugs (default)
- `--fail-on warning` — block on likely issues (recommended for most teams)
- `--fail-on info` — block on any finding (strictest)

To disable failure entirely, pass `--no-fail`.

### Quiet Mode

Add `--quiet` in CI to suppress the banner and progress display, keeping logs clean:

```bash
nitpik review --diff-base main --format github --quiet
```

### Audit Logs as Build Artifacts

Use `--audit-log <PATH>` (or `NITPIK_AUDIT_LOG`) to write a JSON record of the run — per-task status, tool calls, retries, token usage, critic decisions, and final findings — and upload it as a build artifact for after-the-fact debugging:

```yaml
# GitHub Actions
- name: Run nitpik
  run: nitpik review --diff-base origin/main --audit-log nitpik-audit.json --format github
- uses: actions/upload-artifact@v4
  if: always()
  with:
    name: nitpik-audit
    path: nitpik-audit.json
```

## Related Pages

- [Output Formats](09-Output-Formats) — format details and `--fail-on` behavior
- [Configuration](16-Configuration) — environment variables and config files
- [Secret Scanning](14-Secret-Scanning) — enabling secret detection
- [Caching](11-Caching) — how caching saves API cost
