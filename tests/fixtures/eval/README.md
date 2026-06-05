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

## Current baseline & where the headroom is

The committed baseline (`tests/fixtures/eval-baseline.json`) currently sits at
**recall 100% (13/13), precision lower bound ~93%, negatives 7/7 clean**. Recall
is maxed, so the corpus now detects recall *regression* rather than recall
*improvement* — the live headroom is on the **precision lower bound**, driven by
extra warning+ findings on positive fixtures (e.g. `path-traversal-tainted` also
draws an unhandled-I/O finding). The remaining signal to watch:

- **Precision lower bound** — every warning+ finding on an unlabeled line in a
  positive fixture deducts from it. Drives the precision lever.
- **Span/anchor placement** — `resource-leak` and `swallowed-exception` are
  multi-line constructs; their labels carry an `end_line` span so a reviewer
  anchoring anywhere in the construct (the `open()`/`try`, the early return, the
  `except`) is credited. Tightening a span back to a point is how you'd
  re-expose an anchor-placement miss.
- **Verify-panel false-drops** — the perspective-diverse critic panel can in
  principle cut a true-but-borderline finding; the lens prompts now say *drop
  means wrong, not minor*, but watch `verify dropped` counts on positive cases.

Once recall headroom matters again, add harder positives rather than loosening
existing labels. The `--ignored` runner prints each finding's
`file:line [severity] title`, so a miss can be diagnosed (wrong anchor vs. truly
absent) without re-running.

## Roadmap

This corpus is the fast, synthetic, fully-deterministic-to-author baseline.
The planned fidelity upgrade is a curated set of **real merged PRs with known
bugs** — higher signal, but needs human curation. See the PR-native review
plan, Phase A.
