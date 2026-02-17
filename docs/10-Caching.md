# Caching & Prior Findings

nitpik caches review results by content hash, saving time and API cost on unchanged files. When cached results are invalidated, previous findings are carried forward to maintain review consistency.

---

## How Caching Works

nitpik caches review results so that unchanged files aren't re-reviewed. If the same content is reviewed again with the same configuration, nitpik returns the cached findings instantly without calling the LLM.

```bash
# First run — calls the LLM
nitpik review --diff-base main

# Same diff, same config — returns cached results immediately
nitpik review --diff-base main
```

The cache lives at `~/.config/nitpik/cache` by default.

## Prior Findings

When a file changes and its cache entry is invalidated, nitpik carries forward the previous findings to maintain consistency. This prevents flip-flopping — the LLM won't report the same issue differently across runs, and it won't re-flag things you've already fixed.

### Branch Scoping

Prior findings are tracked per **branch**, so parallel PRs reviewing the same file don't cross-contaminate each other. nitpik detects the branch automatically, including in CI environments with detached-HEAD mode (GitHub Actions, GitLab CI, Bitbucket Pipelines, Woodpecker).

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

- [How Reviews Work](09-How-Reviews-Work) — the full review pipeline
- [Configuration](13-Configuration) — all CLI flags
- [CLI Reference](15-CLI-Reference) — `cache` subcommand details
