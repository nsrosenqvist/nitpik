# Review-quality eval corpus

A labeled corpus for measuring nitpik's review quality — recall (do we catch
real issues?) and noise (do we stay quiet on clean code?). Driven by
[`tests/eval.rs`](../../eval.rs).

## Running

The scoring logic is unit-tested under a normal `cargo test`. The end-to-end
scorecard makes real LLM calls and is `#[ignore]`d:

```sh
ANTHROPIC_API_KEY=sk-... cargo test --test eval -- --ignored --nocapture
# also write a baseline file:
NITPIK_EVAL_BASELINE=tests/fixtures/eval-baseline.json \
  ANTHROPIC_API_KEY=sk-... cargo test --test eval -- --ignored --nocapture
```

The scorecard is informational — it never hard-fails on a quality threshold
(real-LLM runs are non-deterministic). Use it to compare prompt/dedup/verify
changes against a baseline.

## Case layout

Each case is a directory with the same `base/` + `changeset/` shape as the
e2e fixtures, plus an `expected.json`:

```
<case>/
  base/<file>        # committed first
  changeset/<file>   # overlaid → this is the diff under review
  expected.json
```

`expected.json`:

```json
{
  "description": "human note on what's planted / why it's clean",
  "kind": "positive | negative",
  "profiles": ["backend"],
  "expected": [
    { "file": "clamp.py", "line": 3, "min_severity": "warning", "note": "off-by-one" },
    {
      "file": "reader.py",
      "line": 2,
      "end_line": 6,
      "min_severity": "warning",
      "keywords": ["leak", "close", "handle"],
      "note": "issue spans lines 2–6; finding must mention one of the keywords"
    }
  ]
}
```

A label's fields:

- `file`, `line` — required anchor.
- `end_line` *(optional)* — last line of the issue's span. The bug may be
  legitimately anchored anywhere in `[line, end_line]` (± the ±2 tolerance),
  for issues that span a region (a resource opened on one line and leaked on
  an early return several lines down). Defaults to a point label at `line`.
- `min_severity` *(optional)* — lowest severity that counts as a catch
  (default `warning`, so an `info` aside doesn't count).
- `keywords` *(optional)* — semantic guard: the finding's title+message must
  contain at least one (case-insensitive) to count. Without it, a finding that
  merely lands on the right line is credited even if it's about an unrelated
  issue — which silently inflates recall and hides a real miss. Add keywords
  whenever a fixture's planted line is a plausible anchor for *other* findings
  too.

## Two kinds of case

- **positive** — one or more *planted* issues at known lines. Scores **recall**:
  a label is "caught" if some finding lands within its line span (± tolerance)
  at `min_severity` or higher and satisfies any `keywords` guard. Extra findings
  matching no label are surfaced; the warning+ ones feed the corpus-wide
  **precision lower bound** (a positive fixture may contain other real issues,
  so it's a lower bound, not an exact precision).
- **negative** — clean-but-tempting code (cosmetic edits, renames, a
  load-bearing guard). `expected` is empty. Any `warning`/`error` finding here
  is **noise**; `info`-level observations are tolerated.

## Adding a case

1. Create `<case>/base/` and `<case>/changeset/` with a minimal file.
2. Keep positives small and focused — ideally one clear issue and little else
   to flag, so precision stays interpretable.
3. Word the planted bug *unlike* the review prompt's own examples, to avoid
   teaching-to-the-test. Include some subtle cases.
4. Write `expected.json`. No code changes needed — the harness auto-discovers
   any directory containing `expected.json`.

## Current baseline & known-hard cases

The committed baseline (`tests/fixtures/eval-baseline.json`) is intentionally
**not** a perfect score — a corpus the reviewer aces every time can't detect
improvement, only regression. The two recurring misses are kept as honest
headroom, not bugs in the corpus:

- **resource-leak** — the planted leak spans the `open()` acquisition and the
  early-return that skips `close()`. The reviewer reliably *finds* it but tends
  to anchor on the `open()` line, which can fall outside a single label's ±2
  window. A signal about anchor placement, not recall.
- **swallowed-exception** — a blanket `except Exception: return 0`. Borderline
  by design: some reviewers treat broad-except-with-default as acceptable, so a
  miss here reflects severity judgment rather than a clear bug.

The `--ignored` runner prints each finding's `file:line [severity] title`, so a
miss can be diagnosed (wrong anchor vs. truly absent) without re-running.

## Roadmap

This corpus is the fast, synthetic, fully-deterministic-to-author baseline.
The planned fidelity upgrade is a curated set of **real merged PRs with known
bugs** — higher signal, but needs human curation. See the PR-native review
plan, Phase A.
