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

The action auto-detects the PR target branch, downloads the binary, and outputs findings as inline annotations on the pull request.

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

**Key details:**
- `fetch-depth: 0` is required for `--diff-base` to have access to the full git history.
- `--format github` outputs findings as workflow commands that appear as inline PR annotations.
- `--fail-on warning` causes the step to fail if any warning or error is found.
- `save-always: true` ensures the cache is persisted even when `--fail-on` causes the step to exit non-zero. Without it, `actions/cache` only saves on job success and the cache is never populated.

> **Security:** Always pass API keys via `${{ secrets.* }}` — never hardcode them in workflow files.

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

**Key details:**
- `--format gitlab` outputs a [Code Quality report](https://docs.gitlab.com/ee/ci/testing/code_quality.html) JSON file.
- Upload it as a `codequality` artifact to see findings in the merge request Code Quality widget.
- Set `XDG_CONFIG_HOME` to a path inside the project directory so the cache is preserved between runs.
- `when: always` ensures the cache is saved even when `--fail-on` causes the job to exit non-zero. Without it, GitLab only saves the cache on success.

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

See [Output Formats — Checkstyle XML](08-Output-Formats#checkstyle-xml) for details.

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

## Related Pages

- [Output Formats](08-Output-Formats) — format details and `--fail-on` behavior
- [Configuration](14-Configuration) — environment variables and config files
- [Secret Scanning](11-Secret-Scanning) — enabling secret detection
- [Caching](10-Caching) — how caching saves API cost
