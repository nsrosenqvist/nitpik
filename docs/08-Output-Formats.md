# Output Formats

nitpik supports seven output formats for different environments — from styled terminal output for local development to structured formats for CI platforms.

---

## Formats Overview

| Format | `--format` value | Use case |
|---|---|---|
| Styled terminal | `terminal` | Local development (default) |
| JSON | `json` | Custom tooling, dashboards, scripts |
| GitHub annotations | `github` | GitHub Actions |
| GitLab Code Quality | `gitlab` | GitLab CI merge request widgets |
| Bitbucket Code Insights | `bitbucket` | Bitbucket Pipelines |
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

See [CI/CD Integration — GitHub Actions](15-CI-Integration#github-actions) for full pipeline setup.

## GitLab Code Quality

```bash
nitpik review --diff-base main --format gitlab > gl-code-quality-report.json
```

Outputs a [GitLab Code Quality report](https://docs.gitlab.com/ee/ci/testing/code_quality.html). Upload it as a CI artifact to see findings in the merge request Code Quality widget.

See [CI/CD Integration — GitLab](15-CI-Integration#gitlab-cicd) for full pipeline setup.

## Bitbucket Code Insights

```bash
nitpik review --diff-base main --format bitbucket
```

Posts findings as [Code Insights annotations](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-reports/) via the Bitbucket API. Inside Bitbucket Pipelines, authentication is handled automatically through the built-in proxy — no token required. Outside Pipelines, set the `BITBUCKET_TOKEN` environment variable with `pullrequest` and `repository:write` scopes.

See [CI/CD Integration — Bitbucket](15-CI-Integration#bitbucket-pipelines) for pipeline config.

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

See [CI/CD Integration — Woodpecker/Forgejo](15-CI-Integration#woodpecker-ci-forgejo--gitea--codeberg) for token setup and pipeline config.

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

- [CI/CD Integration](15-CI-Integration) — full pipeline setup for each platform
- [Configuration](14-Configuration) — `--format` and `--fail-on` in config
- [CLI Reference](16-CLI-Reference) — all output flags
