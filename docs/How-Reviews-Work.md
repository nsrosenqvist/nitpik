# How Reviews Work

nitpik assembles each review to give the LLM the best possible context for precise, actionable findings. Here's what happens when you run a review.

---

## The Review Pipeline

When you run `nitpik review`, the following happens in order:

1. **Diff parsing** — your input (git diff, file scan, patch file, or stdin) is parsed into per-file diffs with hunk boundaries.
2. **Secret scanning** — if `--scan-secrets` is enabled, secrets are detected and redacted before any code leaves your machine.
3. **Context assembly** — for each changed file, nitpik gathers the full file content (or smart excerpts), your project documentation, and prior review findings.
4. **Profile resolution** — your selected profiles are loaded (built-in, custom, auto-detected, or tag-matched).
5. **Task creation** — each (file × profile) pair becomes a review task. Large diffs are split into smaller chunks automatically.
6. **Parallel execution** — tasks are sent to the LLM in parallel (up to `--max-concurrent`, default 5), with exponential backoff on rate limits.
7. **Post-processing** — findings are deduplicated across agents, filtered to diff scope, and severity-normalized.
8. **Output** — formatted and delivered in your chosen format.

## Context Assembly

Each review task includes rich context so the LLM understands the code it's reviewing:

### Full File Content

For files under 1,000 lines (configurable via `max_file_lines`), the LLM sees the entire file — not just the diff. This lets it understand the surrounding code, existing patterns, and the broader context of your change.

For larger files, nitpik extracts the relevant portions: the code surrounding each diff hunk (configurable via `surrounding_lines`), with clear markers showing where content was omitted. This keeps the prompt focused without losing important context.

### Project Documentation

nitpik automatically includes your team's conventions and guidelines. If a `REVIEW.md` or `NITPIK.md` exists in your repo root, those are used as focused review context. Otherwise, nitpik falls back to common documentation files like `AGENTS.md`, `CONVENTIONS.md`, and `CONTRIBUTING.md`.

See [Project Documentation](Project-Docs) for details on controlling this.

### The Diff

The unified diff for the specific file, showing exactly what changed. The LLM is instructed to focus findings only on changed lines.

## Multi-Agent Coordination

When you run multiple profiles together (e.g. `--profile backend,security`), nitpik coordinates them:

- Each reviewer is told which other reviewers are active and what their focus areas are (derived from their tags and descriptions).
- Reviewers are instructed to stay in their lane — a backend reviewer won't duplicate security findings that the security reviewer already covers.
- Each built-in profile also explicitly defines what *not* to report, reinforcing the boundaries.

This coordination happens automatically. You don't need to configure anything — just combine profiles and nitpik handles the rest.

## Prior Findings Continuity

When a file changes and the cached review is invalidated, nitpik doesn't start from scratch. It includes the previous findings in the prompt, with explicit instructions:

- **Re-raise** findings that still apply
- **Drop** findings that have been resolved
- **Add** new findings for newly introduced issues

This keeps reviews consistent across iterations — the LLM won't flip-flop on findings between runs, and it won't re-report issues you've already fixed.

Prior findings are scoped per branch so parallel PRs don't contaminate each other. See [Caching & Prior Findings](Caching) for configuration.

## Post-Processing

Before findings reach you, nitpik applies several quality filters:

### Deduplication

When multiple agents review the same file, they may flag the same issue. nitpik deduplicates by detecting findings that target the same file, overlap on line ranges, and have similar titles. Only the first finding survives.

### Diff Scope Filtering

Findings on lines outside the diff hunks are discarded. This prevents the LLM from flagging pre-existing issues in unchanged code — only your changes are reviewed.

> **Note:** This filter is skipped in `--scan` mode, where the entire file is considered in-scope.

### Severity Normalization

LLMs sometimes use non-standard severity labels. nitpik normalizes them: "critical" and "blocker" become `error`, "major" becomes `warning`, "low" and "minor" become `info`. Unknown labels default to `warning`.

## Related Pages

- [Caching & Prior Findings](Caching) — content-hash caching and prior findings configuration
- [Reviewer Profiles](Reviewer-Profiles) — choosing and combining profiles
- [Agentic Mode](Agentic-Mode) — giving the LLM tools to explore your codebase
- [Secret Scanning](Secret-Scanning) — how redaction works
