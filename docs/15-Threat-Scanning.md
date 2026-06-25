# Threat Scanning

nitpik detects potentially harmful code patterns in your diffs — obfuscated payloads, dangerous API calls, supply chain hooks, backdoors, and data exfiltration — **before** the LLM review even starts. When an LLM provider is configured, flagged patterns are triaged by the model to reduce false positives.

nitpik approaches malicious code in **two complementary layers**:

1. **`--scan-threats`** — fast, deterministic pattern matching (regex + entropy + structural heuristics), then optional LLM triage. Catches known-shape attacks; runs offline (triage aside).
2. **The `malicious` lens** (`--profile malicious`) — an LLM reviewer that reasons about hostile *intent* across the whole diff. Catches what no regex can: a payload split across files, a neutered check, a novel logic bomb.

Run them together for a full [malicious-code scan](#malicious-code-scan-the-malicious-lens).

---

## Enabling Threat Scanning

```bash
nitpik review --diff-base main --scan-threats
```

Or enable it permanently in `.nitpik.toml`:

```toml
[threats]
enabled = true
```

> **Tip:** Combine `--scan-threats` with `--scan-secrets` in CI pipelines for both secret redaction and threat detection in a single pass.

## What Gets Detected

nitpik ships with **44 built-in rules** plus structural heuristics across five threat categories:

| Category | Examples |
|---|---|
| **Obfuscation** | Base64-encoded blobs, hex-encoded strings, `eval` of decoded content, invisible Unicode characters, Hangul filler payloads (Glassworm), mixed-script homoglyph identifiers |
| **Dangerous APIs** | `eval()`, `exec()`, `subprocess` with `shell=True`, `child_process`, `Function()` constructor, `dlopen`/`ctypes` |
| **Supply chain** | `postinstall` script hooks, `pip install` from URLs, `curl \| bash` patterns, typosquatting signals |
| **Exfiltration** | HTTP POSTs to external URLs combined with environment or file reads, credential harvesting patterns |
| **Backdoor** | Reverse shells, hardcoded C2 addresses, socket-based command execution, cron/systemd persistence |

### Unicode and homoglyph attacks

Beyond regex-based rules, nitpik includes dedicated structural checks:

- **Invisible Unicode characters** — zero-width spaces, joiners, fillers, and soft hyphens hidden in source code.
- **Bidi override characters** — Trojan Source attacks (CVE-2021-42574) using Unicode right-to-left overrides to visually reorder code.
- **Mixed-script homoglyph identifiers** — identifiers that mix Latin characters with visually identical Cyrillic or Greek lookalikes (e.g., Cyrillic `р` in `рaypal`). This detects supply chain substitution and credential interception attacks that pass visual review.

## How It Works

Threat scanning runs in two phases:

1. **Pattern matching** — every added line is checked against the built-in rules (regex + entropy + keyword prefilters). Full file contents are also scanned with proximity weighting — matches near changed lines get full severity, distant matches are downgraded to info.
2. **LLM triage** — when an LLM provider is configured, all pattern matches are sent to a single triage call. The model classifies each as confirmed, dismissed, or downgraded. This eliminates false positives from legitimate uses of flagged patterns (e.g., a test file that intentionally contains `eval`).

> **Note:** If the LLM call fails or no provider is configured, all pattern matches pass through as-is — the scanner is fail-open.

## Malicious-code scan: the `malicious` lens

`--scan-threats` is pattern-based: it finds attacks that *look like* known bad shapes. Some malice has no regex tell — the danger is in the **intent and the data flow**, not any single token:

- A secret read in one file and sent out in another (exfiltration split across files).
- A signature or auth check that's quietly turned into a no-op (sabotage by *removing* enforcement).
- A date- or condition-gated destructive payload (a logic bomb using only ordinary APIs).
- A novel obfuscation or staging technique no rule anticipated.

The **`malicious` lens** is an opt-in LLM reviewer for exactly this. It sees the **whole diff at once** (so it can connect behavior across files) and, unlike the `security` lens — which assumes the author made a mistake — it assumes the author may be **adversarial**: a dangerous operation can be the *intended payload*, not an accident.

### Running a full malicious-code scan

Combine the deterministic scanner with the lens to run both layers and **nothing else** — no general quality review:

```bash
nitpik review --diff-base <previous-tag> --profile malicious --scan-threats
```

- `--profile malicious` runs **only** that lens (explicit `--profile` does not add the always-on `security`/`correctness` lenses), so you get a focused malicious-code scan rather than a full review.
- `--scan-threats` adds the deterministic pattern layer and its triage in the same pass.
- The lens needs an **LLM provider**. With none configured, you still get the deterministic layer; the lens simply contributes nothing (fail-open).

This is the recommended shape for gating **untrusted** changes — dependency bumps, first-time-contributor PRs, or scanning each tagged version of a submitted package.

### Use it as a gate

For a publish/merge gate, parse the findings (e.g. [`--format json`](09-Output-Formats)) and decide pass / quarantine / block on severity. Two notes specific to gating:

- **Leave `--verify` off** (the default for this invocation). The verify critic panel can *drop* a borderline finding to reduce false positives — desirable in a PR, but in a gate a dropped finding means a **missed payload**. Treat any drop as "still needs a human," not "cleared."
- **Recall over precision.** The lens leans paranoid: it may flag a constrained dynamic import or a remote-config fetch. For a gate with a human review queue that's an acceptable trade — better a cleared false alarm than a shipped backdoor.

> The `malicious` lens is a **best-effort** layer, like the rest of threat scanning — see [Limitations](#limitations). It is not a guarantee, and findings are advisory.

## Custom Rules

Add your own threat rules:

```bash
nitpik review --diff-base main --scan-threats --threat-rules ./custom-threats.toml
```

Custom rules are loaded in addition to the built-in set. The format:

```toml
[[rules]]
id = "internal-backdoor-pattern"
description = "Detects our known-bad internal pattern"
category = "backdoor"
severity = "error"
scope = "line"
regex = '''connect\(\s*["']internal-c2\.example\.com'''
keywords = ["connect"]
languages = ["py", "js"]

# Optional fields:
# entropy_threshold = 4.0
# min_match_length = 20
# allowlist_paths = ["**/test_*"]
# allowlist_regexes = ["// nosec"]
```

### Rule fields

| Field | Required | Description |
|---|---|---|
| `id` | yes | Unique rule identifier. |
| `description` | yes | Human-readable description shown in findings. |
| `category` | yes | One of: `obfuscation`, `dangerous-api`, `supply-chain`, `exfiltration`, `backdoor`. |
| `severity` | yes | One of: `error`, `warning`, `info`. |
| `scope` | yes | `line` (match individual added lines) or `file` (match full file contents with proximity weighting). |
| `regex` | yes | Pattern to match. Uses Rust `regex` crate syntax with full Unicode support. |
| `keywords` | no | Keyword prefilter — at least one must appear (case-insensitive) before the regex runs. Improves performance on large diffs. Defaults to no prefilter. |
| `languages` | no | File extension filter (e.g., `["py", "js"]`). Empty means all languages. |
| `entropy_threshold` | no | Minimum Shannon entropy for the matched text. Useful for filtering low-entropy false positives on base64/hex rules. Defaults to `0.0` (disabled). |
| `min_match_length` | no | Minimum character length for the matched text. Defaults to `0` (disabled). |
| `allowlist_paths` | no | Glob patterns for files to skip (e.g., `["**/test_*", "docs/**"]`). |
| `allowlist_regexes` | no | Regex patterns — if any match the full line, the finding is suppressed (e.g., `["// nosec", "# noqa"]`). |

## Combining with Secret Scanning

Threat scanning and secret scanning are complementary:

- **Secret scanning** (`--scan-secrets`) detects and **redacts** credentials before code is sent to the LLM.
- **Threat scanning** (`--scan-threats`) detects **malicious patterns** and reports them as findings.

Enable both for maximum coverage:

```bash
nitpik review --diff-base main --scan-secrets --scan-threats
```

```toml
# .nitpik.toml
[secrets]
enabled = true

[threats]
enabled = true
```

## CLI Flags

| Flag | Default | Description |
|---|---|---|
| `--scan-threats` | `false` | Enable threat pattern detection and LLM triage. |
| `--threat-rules <PATH>` | — | Additional threat rules file (TOML format). Loaded alongside built-in rules. |

## Configuration

### `.nitpik.toml`

```toml
[threats]
enabled = true
# additional_rules = "path/to/custom-threats.toml"
```

| Key | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Enable threat scanning by default. Equivalent to always passing `--scan-threats`. |
| `additional_rules` | string | *(none)* | Path to additional threat rules TOML file. |

## Performance

Built-in rules are compiled once per run using parallel regex compilation. The startup cost is comparable to secret scanning (~3–5 seconds). LLM triage adds one additional API call when findings exist.

## Limitations

Threat scanning is a **best-effort defense layer**, not a guarantee. The built-in rules and heuristics cover common attack patterns, but determined attackers can craft payloads that evade regex-based detection. LLM triage reduces false positives but may also dismiss genuine threats.

- **nitpik does not guarantee detection of all malicious code.** Novel obfuscation techniques, zero-day patterns, and sophisticated supply chain attacks may not be caught.
- **Threat findings are advisory.** Always verify flagged patterns with human judgment before acting on them.
- **nitpik is not a replacement for dedicated security tooling** — use it alongside SAST scanners, dependency audits, and manual security review.

## Related Pages

- [Reviewer Profiles & Lenses](06-Reviewer-Profiles#opt-in-lenses) — the `malicious` lens and other opt-in reviewers
- [Secret Scanning](14-Secret-Scanning) — credential detection and redaction
- [How Reviews Work](10-How-Reviews-Work) — where threat scanning fits in the pipeline
- [Configuration](16-Configuration) — full config reference
- [CI/CD Integration](17-CI-Integration) — enabling threat scanning in pipelines
