# Eval corpus

A labeled corpus for measuring nitpik's output quality — recall (do we catch
real issues?) and noise (do we stay quiet on clean code?). Driven by
[`tests/eval.rs`](../../eval.rs).

The corpus is split into two **suites**, each with its own scorecard and its
own committed baseline so a change to one lens can't move the other's gate:

| Suite | Test | Engine | Baseline |
|---|---|---|---|
| `review` (default) | `eval_corpus_scorecard` | shipped default engine (`auto` + `--verify`) | `tests/fixtures/eval-baseline.json` |
| `malicious` | `eval_malicious_scorecard` | the opt-in `malicious` lens in isolation (`--profile malicious`, no verify) | `tests/fixtures/eval-malicious-baseline.json` |

A case declares its suite via the `suite` field in `expected.json` (defaults to
`review`); that determines which scorecard scores it.

## Running

The scoring logic is unit-tested under a normal `cargo test`. The end-to-end
scorecards make real LLM calls and are `#[ignore]`d:

```sh
# review suite (compares to / can rewrite the unsuffixed baseline)
ANTHROPIC_API_KEY=sk-... NITPIK_EVAL_COMPARE=tests/fixtures/eval-baseline.json \
  cargo test --test eval eval_corpus_scorecard -- --ignored --nocapture

# malicious suite — baseline env vars are namespaced with the _MALICIOUS suffix
ANTHROPIC_API_KEY=sk-... NITPIK_EVAL_COMPARE_MALICIOUS=tests/fixtures/eval-malicious-baseline.json \
  cargo test --test eval eval_malicious_scorecard -- --ignored --nocapture

# write a baseline (per suite): NITPIK_EVAL_BASELINE / NITPIK_EVAL_BASELINE_MALICIOUS
```

`NITPIK_EVAL_ONLY=sub1,sub2` narrows either suite to cases whose directory name
contains one of the substrings — cheap single-case iteration without paying for
the whole suite (e.g. `NITPIK_EVAL_ONLY=malicious-logic-bomb`).

The scorecard is informational — it never hard-fails on a quality threshold
(real-LLM runs are non-deterministic) unless `NITPIK_EVAL_STRICT=1` is set. Use
it to compare prompt/dedup/verify/profile changes against a baseline.

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
  "suite": "review",
  "profiles": ["malicious"],
  "force_profiles": false,
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

Case-level fields:

- `kind` — `positive` (planted issues) or `negative` (clean). See below.
- `suite` *(optional, default `review`)* — which scorecard scores this case.
- `profiles` — profiles to run. Only consulted when `force_profiles` is set;
  otherwise the `review` suite runs the shipped `auto` engine regardless.
- `force_profiles` *(optional, default `false`)* — run the declared `profiles`
  in isolation (no `auto`, no `--verify`) instead of the default engine. The
  `malicious` suite uses this to measure the `malicious` lens alone, the way
  the registry invokes it; the `auto` engine never selects that lens.

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

For a **`malicious`-suite** case, set `"suite": "malicious"`, `"profiles":
["malicious"]`, and `"force_profiles": true` so it runs the `malicious` lens in
isolation (the `auto` engine never selects that lens). Positives there target
hostile *intent* a regex can't see (cross-file exfil, neutered checks, logic
bombs); negatives are near-misses of those positives (a legit subprocess, env
read, dynamic import) to keep precision honest.

## Current baselines & where the headroom is

### `malicious` suite (`tests/fixtures/eval-malicious-baseline.json`)

14 cases (9 positives across the intent categories — install-time exfil,
obfuscated eval, auth backdoor, env exfil, logic bomb, cross-file payload,
homoglyph dep, weakened verify, staged remote exec — plus 5 near-miss
negatives). Baseline: **recall 100% (10/10 labels), precision lower bound ~83%,
negatives 3/5 clean**. The two standing false positives are deliberate: the
lens flags a constrained `importlib` plugin load and a remote-config fetch —
acceptable paranoia for a publish *gate* (a human clears them), noise for a PR
lens. They are recorded in the baseline so the gate catches *new* noise or a
recall drop, not so they're treated as correct. Don't tune them out against this
synthetic corpus (teaching-to-the-test); revisit calibration on real malware
samples.

### `review` suite (`tests/fixtures/eval-baseline.json`)

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
