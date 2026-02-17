# Troubleshooting

Common issues and how to resolve them.

---

## Provider Authentication Errors

**Symptom:** "authentication failed", "invalid API key", or "unauthorized" errors.

**Solutions:**
- Verify the correct env var is set for your provider (e.g. `ANTHROPIC_API_KEY` for Anthropic, `OPENAI_API_KEY` for OpenAI). Run `echo $ANTHROPIC_API_KEY` to confirm.
- Check that `NITPIK_PROVIDER` matches the key you've set. A mismatch (e.g. `NITPIK_PROVIDER=openai` with only `ANTHROPIC_API_KEY` set) will fail.
- If using `NITPIK_API_KEY` as a fallback, ensure it's a valid key for the provider specified in `NITPIK_PROVIDER`.
- For `openai-compatible` providers, verify that `NITPIK_BASE_URL` points to the correct endpoint.

## Empty Findings

**Symptom:** nitpik completes successfully but reports zero findings.

**Possible causes:**
- **Cache hit** — the same content was reviewed before. Run with `--no-cache` to force a fresh review.
- **Small or trivial diff** — the LLM may genuinely find no issues. This is expected for simple changes.
- **Wrong diff base** — verify your `--diff-base` ref exists and has diverged from your current branch. Run `git diff main --stat` to confirm there are changes.
- **Model limitations** — some smaller models may miss issues that larger models catch. Try a more capable model.

## Findings Outside Diff Scope

**Symptom:** findings appear on lines you didn't change.

This shouldn't happen in `--diff-base`, `--diff-file`, or `--diff-stdin` modes — nitpik filters findings to diff hunk boundaries after the LLM responds.

In `--scan` mode, the entire file is in scope, so findings on any line are expected.

If you see out-of-scope findings in diff mode, please [report it](https://github.com/nsrosenqvist/nitpik/issues).

## Slow Secret Scanning

**Symptom:** nitpik takes 20-30 seconds before starting the review.

This is expected when `--scan-secrets` is enabled. The 200+ built-in regex rules take time to compile on the first invocation. The cost is paid once per run and does not scale with the number of files.

If you don't need secret scanning for a particular run, omit `--scan-secrets`.

## Stale Cache Data

**Symptom:** nitpik returns outdated findings that don't match the current code.

**Solutions:**
- Run with `--no-cache` to force a fresh review.
- Run `nitpik cache clear` to wipe all cached data.
- Sidecar metadata files older than 30 days are cleaned up automatically, but if you suspect stale data from an old branch, clearing the cache is the fastest fix.

## Docker Permission Issues

**Symptom:** "permission denied" errors when running nitpik in Docker.

**Solutions:**
- Ensure the mounted volume is readable: `-v "$(pwd)":/repo`.
- If the repo is owned by a different user inside the container, git may refuse to operate. The official image configures `safe.directory` for `/repo`, but custom images may not.
- For cache persistence, mount the cache directory: `-v nitpik-cache:/root/.config/nitpik/cache`.

## CI Token Scope Errors

**Symptom:** "forbidden" or "insufficient permissions" when using `--format bitbucket` or `--format forgejo`.

**Bitbucket:**
- The `BITBUCKET_TOKEN` needs `pullrequest` and `repository:write` scopes.
- Create a Repository Access Token under **Repository settings → Access tokens**.

**Forgejo/Gitea:**
- The `FORGEJO_TOKEN` needs at minimum `write:repository` scope.
- Create a personal access token under **User settings → Applications**.

## Self-Update in Containers

**Symptom:** `nitpik update` warns about container environments.

This is intentional. In Docker containers and CI, you should rebuild the image or pin a version in your pipeline instead of self-updating. The binary inside a container image should be immutable.

Pin a specific version:

```bash
docker pull ghcr.io/nsrosenqvist/nitpik:0.2.0
```

## Rate Limiting

**Symptom:** "rate limited" or "too many requests" messages in progress output.

nitpik automatically retries with exponential backoff (up to 5 retries) when rate limited by the LLM provider. If you're consistently hitting rate limits:

- Reduce `--max-concurrent` (default is 5) to lower parallel LLM calls.
- Use fewer profiles per run.
- Check your provider's rate limit tier and consider upgrading.

## Git Errors

**Symptom:** "not a git repository" or "unknown revision" errors.

- `--diff-base` requires a git repository. Use `--scan` for non-git directories.
- Ensure the ref exists: `git rev-parse main` should succeed.
- In CI, ensure `fetch-depth: 0` (GitHub Actions) or `git fetch origin <branch>` so the base ref is available.

## Related Pages

- [Installation](Installation) — install methods and requirements
- [LLM Providers](Providers) — provider setup
- [CI/CD Integration](CI-Integration) — platform-specific setup
- [Caching](Caching) — cache management
