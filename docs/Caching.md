# Caching & Prior Findings

nitpik caches review results by content hash, saving time and API cost on unchanged files. When cached results are invalidated, previous findings are carried forward to maintain review consistency.

---

## How Caching Works

Each review task is cached by a hash of its content — the file, the diff, the prompt context, the agent profile, and the model. If the same content is reviewed again, nitpik returns the cached findings instantly without calling the LLM.

```bash
# First run — calls the LLM
nitpik review --diff-base main

# Same diff, same config — returns cached results immediately
nitpik review --diff-base main
```

The cache lives at `~/.config/nitpik/cache` by default.

## Prior Findings

When a file changes and its cache entry is invalidated, nitpik includes the **previous findings** in the new review prompt. This gives the LLM context about what was already flagged, with instructions to:

- **Re-raise** findings that still apply to the current code
- **Drop** findings resolved by the new changes
- **Add** genuinely new findings

This prevents flip-flopping — the LLM won't report the same issue differently across runs, and it won't re-flag things you've already fixed.

### Branch Scoping

Prior findings are tracked per **branch**, so parallel PRs reviewing the same file don't cross-contaminate each other. nitpik detects the branch from `git rev-parse --abbrev-ref HEAD`, falling back to CI environment variables when in detached-HEAD mode:

- `GITHUB_HEAD_REF`
- `CI_COMMIT_BRANCH` / `CI_MERGE_REQUEST_SOURCE_BRANCH_NAME` (GitLab)
- `BITBUCKET_BRANCH`
- `CI_BRANCH` (Woodpecker)

### Severity Ordering

Previous findings are sorted by severity (errors first) before injection, so the most important context is always preserved — especially useful when `--max-prior-findings` caps the count.

## Configuration

### Disabling the Cache

Skip caching entirely for a single run:

```bash
nitpik review --diff-base main --no-cache
```

When set, every file is re-reviewed even if unchanged. This increases API cost but guarantees fresh findings.

### Disabling Prior Findings

Run a clean-slate review without prior-finding context:

```bash
nitpik review --diff-base main --no-prior-context
```

The cache still works (unchanged files are skipped), but invalidated entries won't include previous findings in the new prompt.

### Limiting Prior Findings

Cap the number of prior findings included in the prompt:

```bash
nitpik review --diff-base main --max-prior-findings 10
```

Useful when previous reviews produced many findings and you want to limit prompt size (and token cost). Findings are sorted by severity before truncation, so errors are always included first.

## Cache Management

```bash
nitpik cache stats   # show entry count and total size
nitpik cache clear   # wipe all cached results and sidecar metadata
nitpik cache path    # print the cache directory path
```

### Stale Cleanup

Sidecar metadata files (which track prior findings per branch) older than **30 days** are automatically removed at the start of each review run. This prevents the cache from growing indefinitely after branches are merged or deleted.

## Config File Options

```toml
# .nitpik.toml
[review]
# No cache-specific config — caching is always on unless --no-cache is passed
```

Caching behavior is controlled entirely through CLI flags — there are no config file options to disable it permanently. This is intentional: caching should be the default, and disabling it should be a conscious per-run decision.

## Related Pages

- [How Reviews Work](How-Reviews-Work) — the full review pipeline
- [Configuration](Configuration) — all CLI flags
- [CLI Reference](CLI-Reference) — `cache` subcommand details
