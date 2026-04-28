# Reviewer Profiles

Profiles are specialist reviewers — each with its own focus areas, system prompt, and severity guidelines. Run one or several in parallel to get targeted feedback from different perspectives.

---

## Built-in Profiles

nitpik ships with five profiles:

### `backend`

Focuses on server-side code quality: correctness, error handling, performance, concurrency, API design, and data integrity. Catches bugs like logic errors, N+1 queries, missing error propagation, and race conditions.

Skips pure formatting issues and deep security analysis — it flags obvious problems (like unsanitized SQL) but leaves thorough security review to the `security` profile.

```bash
nitpik review --diff-base main --profile backend
```

### `frontend`

Focuses on user-facing code: accessibility, rendering performance, state management, UX, and responsive design. Catches missing ARIA labels, improper heading hierarchy, unnecessary re-renders, and memory leaks from unsubscribed listeners.

Skips backend logic and deep security analysis.

```bash
nitpik review --diff-base main --profile frontend
```

### `architect`

Focuses on system design: coupling, abstractions, module boundaries, API surface changes, and backward compatibility. Catches god objects, leaky abstractions, breaking changes, and dependency direction violations.

Skips localized implementation bugs — if a change is purely internal to one function, the architect won't nitpick it unless it reveals a systemic pattern.

```bash
nitpik review --diff-base main --profile architect
```

### `security`

Focuses on vulnerabilities: injection risks (SQL, XSS, command injection), authentication and authorization flaws, cryptographic misuse, data exposure, and insecure configuration. Traces data flow from untrusted input to sensitive sinks.

Reports findings only when the vulnerability path can be verified — no speculative alerts.

```bash
nitpik review --diff-base main --profile security
```

### `general`

A broad, language-agnostic catch-all reviewer. Focuses on universals: correctness, clarity, error handling, configuration mistakes, broken references, and obvious anti-patterns across any file type — documentation, shell scripts, Markdown, configuration formats, and languages without a dedicated specialist.

Used by `auto` mode as the catch-all when no language specialist (`backend` or `frontend`) is selected for the diff. Defers deep domain analysis to specialist reviewers when they run alongside it.

```bash
nitpik review --diff-base main --profile general
```

## Combining Profiles

Run multiple profiles in parallel:

```bash
nitpik review --diff-base main --profile backend,security
```

When multiple profiles run together, each one is informed about the others and their focus areas. This coordination prevents duplicate findings and keeps each reviewer in its lane. See [How Reviews Work](09-How-Reviews-Work) for details.

## Auto-Selection

Let nitpik pick profiles based on the files in your diff:

```bash
nitpik review --diff-base main --profile auto
```

Auto-selection examines three layers of signals to choose profiles:

1. **File extensions and paths** — unambiguous extensions (`.vue`, `.css` → frontend; `.rs`, `.go`, `.py` → backend) are classified directly. JS/TS files are disambiguated using directory names (e.g. `controllers/` → backend, `components/` → frontend) and filename patterns (e.g. `*.controller.ts` → backend).
2. **Project root markers** — when JS/TS path signals are absent or one-sided, nitpik checks the repo root for `package.json` dependencies (Express, React, etc.) and config files (`nest-cli.json`, `wrangler.toml`, etc.) to fill in the gaps.
3. **Architect triggers** — the `architect` profile is added when the diff touches cross-cutting files (CI configs, Dockerfiles, IaC, dependency manifests, API definitions, database migrations) or when the diff is large (many files or many distinct directories).

The `security` profile is always included because its frontmatter sets `always_include: true`. Any custom profile in your `--profile-dir` with the same flag is appended too — see [Always-On Profiles](06-Custom-Profiles#always-on-profiles).

When neither `frontend` nor `backend` signals fire (for example a docs-only, infra-only, or shell-only diff), the `general` profile is used as the catch-all. `general` and the language specialists are mutually exclusive: a strictly-backend or strictly-frontend diff never pulls `general` in alongside the specialist. JS/TS files with no clear signal still default to `frontend` rather than `general`.

## Tag-Based Selection

Select profiles by tag instead of name:

```bash
nitpik review --diff-base main --tag security
nitpik review --diff-base main --tag css,accessibility
```

All profiles (built-in and custom) whose tags contain any of the given values are included. Tag matching is case-insensitive.

Combine `--tag` with `--profile` to add tag-matched profiles on top of explicit ones:

```bash
nitpik review --diff-base main --profile backend --tag css
```

### Built-in Profile Tags

| Profile | Tags |
|---|---|
| `backend` | `backend`, `api`, `database`, `logic`, `performance` |
| `frontend` | `frontend`, `ui`, `ux`, `accessibility`, `css`, `javascript`, `typescript` |
| `architect` | `architecture`, `design`, `patterns`, `maintainability`, `coupling` |
| `security` | `security`, `auth`, `injection`, `xss`, `csrf`, `cryptography` |
| `general` | `docs`, `config`, `shell`, `scripts`, `prose`, `cross-cutting` |

## Listing Profiles

See all available profiles, including custom ones:

```bash
nitpik profiles
nitpik profiles --profile-dir ./agents
```

This shows each profile's name, description, and tags.

## Related Pages

- [Custom Profiles](06-Custom-Profiles) — create your own reviewers
- [How Reviews Work](09-How-Reviews-Work) — multi-agent coordination
- [Agentic Mode](07-Agentic-Mode) — give profiles access to tools
- [CLI Reference](16-CLI-Reference) — all profile-related flags
