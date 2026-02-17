# Output Formats

nitpik supports six output formats for different environments — from styled terminal output for local development to structured formats for CI platforms.

---

## Formats Overview

| Format | `--format` value | Use case |
|---|---|---|
| Styled terminal | `terminal` | Local development (default) |
| JSON | `json` | Custom tooling, dashboards, scripts |
| GitHub annotations | `github` | GitHub Actions |
| GitLab Code Quality | `gitlab` | GitLab CI merge request widgets |
| Bitbucket Code Insights | `bitbucket` | Bitbucket Pipelines |
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

See [CI/CD Integration — GitHub Actions](CI-Integration#github-actions) for full pipeline setup.

## GitLab Code Quality

```bash
nitpik review --diff-base main --format gitlab > gl-code-quality-report.json
```

Outputs a [GitLab Code Quality report](https://docs.gitlab.com/ee/ci/testing/code_quality.html). Upload it as a CI artifact to see findings in the merge request Code Quality widget.

See [CI/CD Integration — GitLab](CI-Integration#gitlab-cicd) for full pipeline setup.

## Bitbucket Code Insights

```bash
nitpik review --diff-base main --format bitbucket
```

Posts findings as [Code Insights annotations](https://developer.atlassian.com/cloud/bitbucket/rest/api-group-reports/) via the Bitbucket API. Requires a `BITBUCKET_TOKEN` environment variable with `pullrequest` and `repository:write` scopes.

See [CI/CD Integration — Bitbucket](CI-Integration#bitbucket-pipelines) for token setup and pipeline config.

## Forgejo / Gitea

```bash
nitpik review --diff-base main --format forgejo
```

Posts findings as inline PR review comments via the Forgejo/Gitea API. Requires a `FORGEJO_TOKEN` environment variable with `write:repository` scope.

See [CI/CD Integration — Woodpecker/Forgejo](CI-Integration#woodpecker-ci-forgejo--gitea--codeberg) for token setup and pipeline config.

## Failing on Findings

Use `--fail-on` to exit with a non-zero status code when findings meet a severity threshold:

```bash
nitpik review --diff-base main --format github --fail-on warning
```

| `--fail-on` value | Exits non-zero when |
|---|---|
| `error` | Any finding has severity `error` |
| `warning` | Any finding has severity `warning` or `error` |
| `info` | Any finding exists (any severity) |

When not set, nitpik always exits `0` regardless of findings. In CI, combine `--fail-on` with your platform's failure handling to block merges on review findings.

## Related Pages

- [CI/CD Integration](CI-Integration) — full pipeline setup for each platform
- [Configuration](Configuration) — `--format` and `--fail-on` in config
- [CLI Reference](CLI-Reference) — all output flags
