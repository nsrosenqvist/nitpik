# Project Documentation Context

nitpik automatically detects documentation files in your repository and includes them in the review prompt, so the LLM understands your team's conventions, architecture, and coding standards.

---

## How It Works

Before each review, nitpik scans your repo root for well-known documentation files and includes their content in the prompt alongside the diff and file content. This gives the LLM context about your team's expectations — naming conventions, error handling patterns, architectural decisions — so findings align with how your team actually works.

## Priority Files: `REVIEW.md` and `NITPIK.md`

If a `REVIEW.md` or `NITPIK.md` file exists in your repo root, nitpik uses **only** those files as review context and skips the generic doc list entirely.

Think of `REVIEW.md` as an `AGENTS.md` for nitpik — a focused file that tells the reviewer exactly what your team cares about, without the noise of general contributor docs.

Both files can coexist — if both are present, both are included.

### Writing an Effective `REVIEW.md`

Keep it focused on what matters for code review. A good `REVIEW.md` is short and specific:

```markdown
# Review Guidelines

## Error Handling
- Use `anyhow::Result` in CLI code, `thiserror` enums in library modules.
- No `.unwrap()` outside tests.

## Naming
- `snake_case` for functions, `PascalCase` for types.
- Prefix boolean variables with `is_` or `has_`.

## Performance
- All database queries must use parameterized statements.
- Avoid allocations in hot loops — prefer iterators.

## What We Don't Care About
- Import ordering (handled by rustfmt).
- Line length (handled by clippy).
```

Don't duplicate your entire `CONTRIBUTING.md` here — focus on the conventions that a code reviewer should enforce.

## Fallback Documentation

When no `REVIEW.md` or `NITPIK.md` is found, nitpik falls back to scanning for these common documentation files:

- `AGENTS.md`
- `ARCHITECTURE.md`
- `CONVENTIONS.md`
- `CONTRIBUTING.md`
- `CLAUDE.md`
- `.github/copilot-instructions.md`
- `.cursorrules`
- `CODING_GUIDELINES.md`
- `STYLE_GUIDE.md`
- `DEVELOPMENT.md`

Files larger than **256 KB** are skipped to keep prompt size manageable.

## Controlling Project Docs

### Skip All Documentation

Suppress all auto-detected project docs:

```bash
nitpik review --diff-base main --no-project-docs
```

This reduces prompt size and token cost, but the LLM won't know about your team's conventions. Useful for quick one-off reviews or when testing.

### Exclude Specific Files

Keep most docs but exclude noisy ones:

```bash
nitpik review --diff-base main --exclude-doc AGENTS.md
nitpik review --diff-base main --exclude-doc AGENTS.md,CONTRIBUTING.md
```

> **Note:** Excluding all priority files (e.g. `--exclude-doc REVIEW.md,NITPIK.md`) causes nitpik to fall back to the generic doc list.

## Related Pages

- [How Reviews Work](How-Reviews-Work) — where project docs fit in the prompt
- [Custom Profiles](Custom-Profiles) — profile-level conventions
- [Configuration](Configuration) — `--no-project-docs` and `--exclude-doc` flags
