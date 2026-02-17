---
name: write-docs
description: Write and maintain user-facing documentation for the nitpik project. Use this skill when creating, editing, or reviewing any Markdown documentation under /docs or in the README. It defines tone, structure, and formatting conventions for consistency across all documentation pages.
---

# Documentation Writing Guide for nitpik

Follow these conventions when writing or editing any user-facing documentation (README.md, /docs wiki pages, inline help text).

## Voice & Tone

- **Second person, active voice.** Address the reader as "you." Say "nitpik diffs your branch" not "the branch is diffed."
- **Confident and concise.** State facts directly. Avoid hedging ("you might want to", "it is possible to"). Prefer "Use `--agent`" over "You can use `--agent`."
- **Practical, not academic.** Lead with what the user can *do*, not how the system works internally. Implementation details belong in AGENTS.md, not in user docs.
- **Friendly but professional.** No jokes or slang. Contractions are fine (don't, isn't, you'll).

## Structure

- **One topic per page.** Each doc page covers a single concern. Don't combine "Caching" with "Secret Scanning."
- **Lead with the payoff.** Start each page with a one-sentence summary of what the feature does for the user, then explain how to use it.
- **Progressive disclosure.** Put the simplest usage first, then layer in options and edge cases. The 80% use case goes at the top; the 20% advanced usage goes later.
- **Use headings liberally.** Readers scan — make every section findable. Use `##` for major sections, `###` for subsections.
- **Cross-link related pages.** At the bottom of each page or inline where relevant, link to related topics. Use wiki-style links: `[Page Title](Page-Title)`.

## Formatting Conventions

- **Code blocks** for all CLI commands, config snippets, and file content. Use `bash` for shell commands, `toml` for config, `markdown` for profile examples, `yaml` for CI pipelines.
- **Tables** for comparing options, listing flags, or showing env vars. Keep tables compact — if a cell needs more than one sentence, use a description list or subsection instead.
- **Bold** for the first mention of a key term or UI element. Don't bold the same term repeatedly.
- **Inline code** for flag names (`--profile`), env vars (`NITPIK_API_KEY`), file names (`REVIEW.md`), command names (`nitpik review`), and config keys (`fail_on`).
- **Admonitions** use blockquotes with a bold lead: `> **Note:** ...`, `> **Tip:** ...`, `> **Security:** ...`, `> **Warning:** ...`.

## Content Rules

- **Document effects, not just existence.** Don't just say a flag exists — explain what changes when you set it. "When `--no-cache` is set, every file is re-reviewed even if unchanged, which increases API cost but guarantees a fresh review."
- **Show before telling.** Put a short example command or config snippet before (or immediately after) explaining what it does.
- **One canonical example per concept.** Don't show five ways to do the same thing unless distinguishing between them is the point of the section.
- **Keep security advice concrete.** Say "Pass API keys via `${{ secrets.* }}` — never hardcode them in workflow files" not "Be careful with secrets."
- **Default values always mentioned.** When documenting a flag or config key, state the default. "Defaults to `terminal`."
- **Flag and config correspondence.** When a CLI flag has a config-file and/or env-var equivalent, mention all three together so readers can find whichever they need.

## Page Template

Each wiki page should follow this general shape:

```
# Page Title

One-sentence summary of what this feature does for the user.

---

## Primary Usage

Simplest, most common way to use the feature. Include a code example.

## Options / Flags

Detailed coverage of all relevant flags, config keys, and env vars.

## Advanced Usage

Edge cases, combinations, less common workflows.

## Related Pages

- [Related Topic](Related-Topic)
```

Not every page needs every section — adapt to the content. Skip "Advanced Usage" if there's nothing advanced to say.

## Spelling & Terminology

- American English throughout (behavior, not behaviour; sanitization, not sanitisation).
- "nitpik" is always lowercase, even at the start of a sentence.
- "LLM" not "AI model" or "language model" (except on first mention where you can expand it).
- "finding" not "issue" or "result" when referring to review output.
- "profile" not "agent" when referring to reviewer configurations (except when discussing agentic mode).
- "provider" for the LLM service (Anthropic, OpenAI, etc.).
