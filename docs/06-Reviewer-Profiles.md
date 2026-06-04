# Reviewer Profiles & Lenses

nitpik reviews a diff through **lenses** — issue-typed specialist reviewers, each hunting one orthogonal class of problem (correctness, security, concurrency, …). Several run in parallel and their findings are merged, deduplicated, and verified.

By default nitpik runs `--profile auto`: two always-on lenses plus the conditional lenses a triage step judges the change actually warrants. Pass `--profile <name>` to run an exact set instead.

---

## How the default review works

Every default review runs the two **always-on lenses**:

| Lens | Hunts for |
|---|---|
| `security` | injection, authZ, secrets, input validation, crypto, cross-tenant isolation |
| `correctness` | logic bugs, off-by-one, error handling, edge cases, invariant/state-machine violations |

On top of those, a fast **triage** step selects from the **conditional lenses** — only the ones whose failure mode the change plausibly contains:

| Lens | Hunts for | Scope |
|---|---|---|
| `concurrency` | races, deadlocks, shared mutable state, ordering | per file |
| `performance` | N+1 queries, hot-path allocation, complexity, latency | per file |
| `test-integrity` | coverage of changed behavior, determinism, tautological tests | per file |
| `operational` | observability, migrations (fwd+rollback), feature flags, config/secrets | per file |
| `a11y` | semantics, ARIA, keyboard nav/focus, contrast, labels | per file |
| `user-journey` | UX happy-path + failure-mode walkthrough | per file |
| `contract-impact` | rename/remove/signature ripple to call sites; cross-file API/back-compat | whole diff |
| `docs-drift` | docs/comments/OpenAPI/changelog no longer matching changed behavior | whole diff |
| `holistic` | whole-PR coherence; symmetric obligations (rollback-for-migration) | whole diff |

It is normal for triage to pick **no** conditional lens on a small, self-contained change — the always-on lenses already cover it. The three whole-diff lenses (`contract-impact`, `docs-drift`, `holistic`) review the entire change set at once and use repository-exploration tools by default (see [Agentic Mode](08-Agentic-Mode)).

## Auto-Selection

When `--profile` is omitted, nitpik runs `auto`. You can request it explicitly:

```bash
nitpik review --diff-base main --profile auto
```

The always-on lenses always run. The conditional lenses are chosen by `--auto-mode`:

| Mode | Behavior |
|---|---|
| `heuristic` | File/path rules only — no LLM call. Maps frontend files → `a11y`/`user-journey`, tests → `test-integrity`, structural/large diffs → `operational`/`contract-impact`/`holistic`, docs → `docs-drift`. Fully offline. |
| `llm` | Always ask the model to pick conditional lenses by substance, using the built-in `triage` prompt. |
| `hybrid` (default) | Consults the LLM for substance-based selection on every run; falls back to the heuristic if the triage call can't run or fails. |

Triage is a cheap call — point it at a smaller model with `[provider.models] triage` or `NITPIK_TRIAGE_MODEL` (see [Providers](03-Providers#per-task-model-overrides)).

## Explicit selection

`--profile <names>` runs **exactly** what you name — the always-on lenses are **not** added on top. This keeps explicit selection predictable (and is the power-user/CI path), but note that `--profile my-lens` alone runs without the security net; add it back with `--profile my-lens,security,correctness`.

```bash
nitpik review --diff-base main --profile security,correctness,concurrency
```

When multiple lenses run together, each is told what the others cover so they stay in their lane and avoid duplicate findings. See [How Reviews Work](10-How-Reviews-Work).

> **Upgrading from 2.x?** The domain profiles `backend`, `frontend`, `architect`, and `general` were removed in 3.0. Use the issue-typed lenses above instead — for most setups the default `auto` is the direct replacement. Custom profiles you ship in `--profile-dir` are unaffected.

## Tag-Based Selection

Select profiles by tag instead of name; matching is case-insensitive and spans built-in lenses and custom profiles:

```bash
nitpik review --diff-base main --tag accessibility
nitpik review --diff-base main --tag performance,concurrency
```

Combine `--tag` with `--profile` to add tag-matched reviewers on top of an explicit set.

## Always-on and custom profiles

`security` and `correctness` set `always_include: true`, which is why they run on every default review. Any custom profile in your `--profile-dir` with the same flag rides along too; a custom profile that sets `auto_candidate: true` joins the conditional triage pool. See [Custom Profiles](07-Custom-Profiles#always-on-profiles).

## Listing Profiles

```bash
nitpik profiles
nitpik profiles --profile-dir ./agents
```

Shows each profile's name, description, and tags.

## Related Pages

- [Custom Profiles](07-Custom-Profiles) — create your own reviewers
- [How Reviews Work](10-How-Reviews-Work) — multi-agent coordination
- [Agentic Mode](08-Agentic-Mode) — `--agent` policy and tool access
- [CLI Reference](18-CLI-Reference) — all profile-related flags
