# Output Formats

nitpik supports ten output formats for different environments — from styled terminal output for local development to structured formats and real PR/MR reviews for CI platforms.

---

## Formats Overview

| Format | `--format` value | Use case |
|---|---|---|
| Styled terminal | `terminal` | Local development (default) |
| JSON | `json` | Custom tooling, dashboards, scripts |
| GitHub annotations | `github` | GitHub Actions |
| GitHub PR review | `github-pr-review` | Real inline PR review on GitHub |
| GitLab Code Quality | `gitlab` | GitLab CI merge request widgets |
| GitLab MR review | `gitlab-mr-review` | Real inline merge-request review on GitLab |
| Bitbucket Code Insights | `bitbucket` | Bitbucket Pipelines |
| Bitbucket PR review | `bitbucket-pr-review` | Real inline pull-request review on Bitbucket |
| Checkstyle XML | `checkstyle` | Any CI platform with checkstyle support |
| Forgejo/Gitea PR review | `forgejo` | Woodpecker CI, Forgejo, Gitea |

## Terminal (Default)

```bash
nitpik review --diff-base main
```

Styled, human-readable output with color-coded severities. Includes a banner, progress display, and findings summary. Use `--quiet` to suppress everything except findings and errors.

## JSON

```bash
nitpik review --diff-base main --format json
```

Outputs a JSON object with a `findings` array. Each finding contains `file`, `line`, `end_line`, `severity`, `title`, `message`, `suggestion`, and `agent`. Suitable for piping into `jq`, custom dashboards, or downstream tools.

## GitHub Actions

```bash
nitpik review --diff-base main --format github
```

Outputs findings as [GitHub Actions workflow commands](https://docs.github.com/en/actions/writing-workflows/choosing-what-your-workflow-does/workflow-commands-for-github-actions) (`::error::`, `::warning::`, `::notice::`). These appear as inline annotations on pull requests.

See [CI/CD Integration — GitHub Actions](17-CI-Integration#github-actions) for full pipeline setup.

## GitLab Code Quality

```bash
nitpik review --diff-base main --format gitlab > gl-code-quality-report.json
```

Outputs a [GitLab Code Quality report](https://docs.gitlab.com/ee/ci/testing/code_quality.html). Upload it as a CI artifact to see findings in the merge request Code Quality widget.

See [CI/CD Integration — GitLab](17-CI-Integration#gitlab-cicd) for full pipeline setup.

## GitLab MR Review

```bash
nitpik review --diff-base "$CI_MERGE_REQUEST_DIFF_BASE_SHA" --format gitlab-mr-review
```

Posts a **real inline merge-request review** — a summary note plus a positioned discussion on each finding's line — instead of a Code Quality artifact. Like `github-pr-review`, it is dedup-aware (a re-run only posts findings not already raised) and can request changes via `--request-changes`.

Runs in a merge-request pipeline (`CI_MERGE_REQUEST_IID` set) and authenticates with `GITLAB_TOKEN` (a project/personal token with `api` scope) or the pipeline's `CI_JOB_TOKEN`. Choose this over `gitlab` when you want conversational inline review comments rather than the Code Quality widget.

## Bitbucket Code Insights

```bash
nitpik review --diff-base main --format bitbucket
```

Posts findings as [Code Insights annotations](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-reports/) via the Bitbucket API. Inside Bitbucket Pipelines, authentication is handled automatically through the built-in proxy — no token required. Outside Pipelines, set the `BITBUCKET_TOKEN` environment variable with `pullrequest` and `repository:write` scopes.

See [CI/CD Integration — Bitbucket](17-CI-Integration#bitbucket-pipelines) for pipeline config.

## Bitbucket PR Review

```bash
nitpik review --diff-base origin/main --format bitbucket-pr-review
```

Posts a **real inline pull-request review** — a summary comment plus a comment on each finding's line — instead of commit-level Code Insights annotations. Like `github-pr-review`, it is dedup-aware (a re-run only posts findings not already raised) and supports `--request-changes`.

Runs on a **PR-triggered pipeline** (`BITBUCKET_PR_ID` set) and authenticates with a `BITBUCKET_TOKEN` access token that has pull-request write scope (the in-Pipelines proxy used by `bitbucket` Code Insights does not cover PR comments). Choose this over `bitbucket` when you want conversational inline review comments rather than the Code Insights panel.

## Checkstyle XML

```bash
nitpik review --diff-base main --format checkstyle > checkstyle-report.xml
```

Outputs findings in the standard [Checkstyle XML format](https://checkstyle.sourceforge.io/). Each finding maps to a `<error>` element with `severity`, `message`, and `source` attributes.

Checkstyle XML is a universal interchange format supported across the CI ecosystem. Use it when your platform doesn't have a dedicated nitpik output format, or when you want a file-based approach without API calls:

| Platform | How to consume checkstyle XML |
|---|---|
| **Bitbucket Pipelines** | Use the [Checkstyle Code Insight Report pipe](https://bitbucket.org/product/features/pipelines/integrations?search=checkstyle) to display findings as Code Insights annotations |
| **Jenkins** | The [Warnings Next Generation](https://plugins.jenkins.io/warnings-ng/) plugin natively ingests checkstyle XML |
| **Any platform** | [reviewdog](https://github.com/reviewdog/reviewdog) accepts checkstyle XML via `-f=checkstyle` and posts annotations to GitHub, GitLab, Bitbucket, Gitea, and more |

> **Tip:** If your CI platform already has a dedicated nitpik format (`github`, `gitlab`, `bitbucket`, `forgejo`), prefer that — it provides tighter integration. Use `checkstyle` for platforms without a dedicated format, for local tooling, or when you want a portable file you can process downstream.

## Forgejo / Gitea

```bash
nitpik review --diff-base main --format forgejo
```

Posts findings as inline PR review comments via the Forgejo/Gitea API. Requires a `FORGEJO_TOKEN` environment variable with `write:repository` scope.

See [CI/CD Integration — Woodpecker/Forgejo](17-CI-Integration#woodpecker-ci-forgejo--gitea--codeberg) for token setup and pipeline config.

## Failing on Findings

By default, nitpik exits with a non-zero status code when any finding has severity `error` — matching the behavior of standard testing and linting tools like PHPUnit, Vitest, and ESLint.

Override the threshold with `--fail-on`:

```bash
nitpik review --diff-base main --format github --fail-on warning
```

| `--fail-on` value | Exits non-zero when |
|---|---|
| `error` (default) | Any finding has severity `error` |
| `warning` | Any finding has severity `warning` or `error` |
| `info` | Any finding exists (any severity) |

To disable failure entirely (always exit `0`), use `--no-fail`:

```bash
nitpik review --diff-base main --no-fail
```

## Related Pages

- [CI/CD Integration](17-CI-Integration) — full pipeline setup for each platform
- [Configuration](16-Configuration) — `--format` and `--fail-on` in config
- [CLI Reference](18-CLI-Reference) — all output flags
