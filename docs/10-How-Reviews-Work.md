# How Reviews Work

nitpik assembles each review to give the LLM the best possible context for precise, actionable findings. Here's what happens when you run a review.

---

## Overview

When you run `nitpik review`, nitpik parses your diff, gathers relevant context for each changed file, runs your chosen reviewer profiles in parallel, and delivers deduplicated findings in your chosen output format.

If `--scan-secrets` is enabled, secrets are detected and redacted **before** any code is sent to the LLM. See [Secret Scanning](14-Secret-Scanning) for details.

If `--scan-threats` is enabled, nitpik also scans for malicious patterns (obfuscation, dangerous APIs, supply chain hooks, backdoors) and optionally triages them with the LLM. See [Threat Scanning](15-Threat-Scanning) for details.

## Context

nitpik doesn't just send raw diffs to the LLM — it includes surrounding file content and your project documentation so the reviewer understands what it's looking at.

### File Content

For reasonably sized files, the LLM sees the full file alongside the diff. This lets it understand existing patterns, naming conventions, and the broader context of your change. For very large files, nitpik includes the relevant portions surrounding each change.

You can tune this with `max_file_lines` and `surrounding_lines` in your config.

### Project Documentation

nitpik automatically includes your team's conventions and guidelines. If a `REVIEW.md` or `NITPIK.md` exists in your repo root, those are used as focused review context. Otherwise, nitpik falls back to common documentation files like `AGENTS.md`, `CONVENTIONS.md`, and `CONTRIBUTING.md`.

See [Project Documentation](12-Project-Docs) for details on controlling this.

### Commit History

When reviewing a `--diff-base` ref, nitpik includes the commit log (up to 50 commits, newest first) so the LLM understands the author's intent behind the changes. Commit messages like "fix SQL injection in login" help the reviewer verify fixes rather than re-flagging resolved issues.

This only applies to git ref diffs — stdin, file, and scan modes have no commit history. Use `--no-commit-context` to skip it.

## Multi-Agent Coordination

When you run multiple profiles together (e.g. `--profile security,performance`), nitpik automatically coordinates them to avoid duplicate findings. Each reviewer focuses on its own area of expertise without stepping on the others.

You don't need to configure anything — just combine profiles and nitpik handles the rest.

### Multi-Wave Reviews

With `--multi-wave`, profiles whose YAML frontmatter declares `wave: 2` run **after** wave 1 completes and receive a compact summary of wave-1 findings as additional context. This lets late-stage reviewers (e.g. an architect) react to issues found by earlier specialists. Capped at 2 waves; off by default.

### Auto Profile Selection

`--profile auto` runs the always-on lenses (`security`, `correctness`) and selects conditional lenses from the diff itself. The selection strategy is controlled by `--auto-mode`:

- `heuristic` — file/path rules map signals to lenses, no LLM call.
- `llm` — ask the model (using the built-in `triage` profile) to pick conditional lenses by substance.
- `hybrid` (default) — consult the LLM every run; fall back to the heuristic if the triage call can't run or fails.

See [Reviewer Profiles & Lenses](06-Reviewer-Profiles) for the full lens set.

## Verification (Critic Pass)

With `--verify`, nitpik runs a single extra LLM call after deduplication that votes keep/drop on each finding using a built-in `critic` profile. Findings the critic drops are removed from the output; the rest pass through unchanged. The critic fails open — a provider error keeps every finding.

Use `--show-dropped` to print the dropped findings (with rationale) to stderr for debugging.

## Prior Findings

When a file changes and the cached review is invalidated, nitpik carries forward the previous findings so reviews stay consistent across iterations. The LLM won't flip-flop on findings between runs, and it won't re-report issues you've already fixed.

Prior findings are scoped per branch so parallel PRs don't contaminate each other. See [Caching & Prior Findings](11-Caching) for configuration.

## Post-Processing

Before findings reach you, nitpik applies quality filters:

- **Deduplication** — when multiple agents review the same file and flag the same issue, duplicates are removed automatically. Findings can include short `evidence` snippets (the actual code or symbol they reference); shared evidence is one of the signals used to merge near-duplicates.
- **Diff scope filtering** — findings on lines outside the diff are discarded, so only your changes are reviewed. This filter is skipped in `--scan` mode, where the entire file is in scope.
- **Severity normalization** — LLMs sometimes use inconsistent severity labels. nitpik normalizes them to a standard set (`error`, `warning`, `info`).
- **Critic pass** — optional `--verify` step that drops probable false positives. See [Verification](#verification-critic-pass) above.

## Related Pages

- [Caching & Prior Findings](11-Caching) — content-hash caching and prior findings configuration
- [Reviewer Profiles](06-Reviewer-Profiles) — choosing and combining profiles
- [Agentic Mode](08-Agentic-Mode) — giving the LLM tools to explore your codebase
- [Secret Scanning](14-Secret-Scanning) — how redaction works
- [Threat Scanning](15-Threat-Scanning) — malicious pattern detection
