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
    { "file": "clamp.py", "line": 3, "min_severity": "warning", "note": "off-by-one" }
  ]
}
```

## Two kinds of case

- **positive** — one or more *planted* issues at known lines. Scores **recall**:
  a label is "caught" if some finding lands within ±2 lines at `min_severity`
  or higher. Extra findings (matching no label) are surfaced but not penalized,
  since a positive fixture may contain other real issues.
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

## Roadmap

This corpus is the fast, synthetic, fully-deterministic-to-author baseline.
The planned fidelity upgrade is a curated set of **real merged PRs with known
bugs** — higher signal, but needs human curation. See the PR-native review
plan, Phase A.
