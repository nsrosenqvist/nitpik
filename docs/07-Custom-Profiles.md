# Custom Profiles

Create your own reviewer profiles to enforce your team's specific conventions, focus on domain-specific concerns, or integrate custom tools into the review process.

---

## Profile Format

A profile is a Markdown file with YAML frontmatter. The frontmatter defines metadata; the body is the system prompt that instructs the LLM.

```markdown
---
name: team-conventions
description: Enforces our internal coding standards
tags: [style, conventions]
---

You are a code reviewer enforcing our team's conventions.

Check for:
- snake_case functions, PascalCase types
- Result-based error handling (no unwrap in production)
- Doc comments on all public items
```

## Frontmatter Fields

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Unique identifier for the profile. Used with `--profile`. |
| `description` | Yes | Short description of what the profile reviews. Shown in `nitpik profiles` output and used in multi-agent coordination to tell other reviewers what this one covers. |
| `tags` | No | List of tags for `--tag` selection. Also used in coordination notes — when multiple profiles run together, each one sees the other profiles' tags to understand their focus areas. |
| `model` | No | Override the global model for this profile. Useful for using a more capable model on security reviews or a cheaper one for style checks. |
| `agentic_instructions` | No | Additional instructions injected only in `--agent` mode. Use this to tell the LLM how to use tools effectively for this profile's focus. Not included in standard (non-agentic) reviews. |
| `environment` | No | List of env var names (or prefix globs like `AWS_*`) that custom command tools are allowed to inherit. See [Environment Passthrough](#environment-passthrough). |
| `always_include` | No | When `true`, the profile runs on every `auto` review regardless of triage. Defaults to `false`. See [Always-On Profiles](#always-on-profiles). |
| `auto_candidate` | No | When `true`, the profile joins the conditional **triage candidate pool** — `auto` mode may select it by diff substance, like the built-in lenses. Defaults to `false` (the profile runs only when named via `--profile`/`--tag`). |
| `scope` | No | `chunk` (default) reviews one diff chunk per task; `diff` runs the profile once over the **whole change set** — for cross-cutting concerns (rename ripple, docs drift, whole-PR coherence). |
| `agentic` | No | When `true`, this reviewer requests repository-exploration tools by default. The run-level `--agent` policy (`auto`/`on`/`off`) can override it. See [Agentic Mode](08-Agentic-Mode). |
| `wave` | No | Either `1` (default) or `2`. Profiles set to `wave: 2` run after wave 1 completes when `--multi-wave` is enabled, and receive a summary of wave-1 findings as additional context. Ignored without `--multi-wave`. |
| `tools` | No | Custom CLI tools the LLM can invoke in agentic mode. See [Custom Agentic Tools](#custom-agentic-tools). |

## Using Custom Profiles

Reference a profile by file path:

```bash
nitpik review --diff-base main --profile ./team-conventions.md
```

Or place profiles in a directory and reference by name:

```bash
nitpik review --diff-base main --profile-dir ./agents --profile team-conventions
```

Combine custom profiles with built-in ones:

```bash
nitpik review --diff-base main --profile-dir ./agents --profile performance,team-conventions
```

## Overriding Built-In Lenses

A custom profile whose `name` matches a built-in lens (`security`, `correctness`, `performance`, `a11y`, …) replaces that lens when `--profile-dir` is set. This lets you tune a shipped lens to your team's needs without forking the project.

For example, drop a file at `./agents/performance.md`:

```markdown
---
name: performance
description: Performance review tuned for our Rust services
tags: [performance, latency, rust]
---

You are reviewing for performance regressions in our hot paths...
```

Then run:

```bash
nitpik review --diff-base main --profile-dir ./agents --profile performance
```

nitpik loads your `performance.md` instead of the built-in. The override applies everywhere the lens is referenced — `--profile performance`, `--tag performance`, auto-selection, and `nitpik profiles` all use your version.

> **Tip:** Use `nitpik profiles --profile-dir ./agents` to confirm which version of a profile will be used. Overridden built-ins appear once, with your custom description.

## Validating Profiles

Check a profile for syntax errors before using it:

```bash
nitpik validate ./team-conventions.md
```

This verifies the YAML frontmatter structure, required fields, and tool definitions.

## Writing Effective System Prompts

The body of your profile is the system prompt — it shapes everything the LLM focuses on. A few guidelines:

1. **State the reviewer's role.** Start with "You are a [role] performing a code review focused on [area]."
2. **List focus areas explicitly.** Numbered lists with bold headings work well.
3. **Define severity levels.** Tell the LLM what qualifies as `error`, `warning`, and `info` for your specific concerns.
4. **Say what NOT to report.** This is just as important — it prevents noise and keeps the reviewer in its lane when running alongside other profiles.
5. **Be specific to your team.** Reference your actual conventions, libraries, and patterns. "Use `anyhow` for error handling in CLI code" is better than "use proper error handling."

See the [built-in profiles](https://github.com/nsrosenqvist/nitpik/tree/main/src/agents/builtin) for real examples.

## Always-On Profiles

The built-in `security` and `correctness` lenses already run on every change. Teams often want similar always-on coverage for a project-specific concern — license headers, telemetry conventions, a house style rule, or a high-stakes subsystem.

Set `always_include: true` in a profile's frontmatter to make it part of every `auto` review:

```markdown
---
name: license-headers
description: Every new source file carries the SPDX license header
tags: [license, compliance]
always_include: true
---

You are a license-compliance reviewer.

For every newly added source file in the diff, check that it begins with the
project's SPDX license header. Report a finding when a new file is missing it.

- `warning` — a new source file has no license header.
- Don't report on test fixtures, generated files, or non-source assets.
```

Use it like any other profile — drop the file in your `--profile-dir` and `auto` picks it up automatically:

```bash
nitpik review --diff-base main --profile-dir ./agents --profile auto
```

> **Tip:** for a reviewer that should run *conditionally* — only when the diff's substance calls for it, like the built-in lenses — set `auto_candidate: true` instead of `always_include: true`. Triage then selects it by diff substance rather than running it every time.

> **Note:** `always_include`/`auto_candidate` only apply to `auto` mode. Explicit `--profile` and `--tag` selections stay literal — if you list profiles by name, only those run.

### Disabling a Built-In Always-On Profile

The shipped `security` profile sets `always_include: true` so every `auto` review gets a security pass. To opt out (or replace it with your own version), drop a `security.md` override into your `--profile-dir` and set `always_include: false`:

```markdown
---
name: security
description: Security review handled by our external scanner
tags: []
always_include: false
---

You are a security reviewer. Only flag issues not already caught by our SAST pipeline.
```

The override replaces the built-in entirely (see [Overriding Built-In Profiles](#overriding-built-in-profiles)), so the always-on inclusion is removed along with the built-in's prompt.

## Custom Agentic Tools

Profiles can define CLI tools that the LLM can invoke during `--agent` reviews:

```markdown
---
name: test-aware-reviewer
description: Reviews code and can run the test suite
tools:
  - name: run_tests
    description: Run the test suite with an optional filter
    command: cargo test
    parameters:
      - name: filter
        type: string
        description: Test name filter
        required: false
---

You are a reviewer that validates changes against the test suite...
```

### Tool Definition Fields

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Tool identifier the LLM uses to invoke it. |
| `description` | Yes | Tells the LLM when and why to use this tool. Be specific. |
| `command` | Yes | Shell command to execute. Parameters are appended as arguments. |
| `parameters` | No | List of parameters the LLM can pass. Each has `name`, `type`, `description`, and `required`. |

### Sandboxing and Limits

Custom commands run in a sandboxed subprocess with these constraints:

| Limit | Value |
|---|---|
| Timeout | 120 seconds |
| Output cap | 256 KB |
| Virtual memory | 1 GB |
| File write size | 100 MB |

Commands are sandboxed to the repository root. If the LLM passes parameter names not declared in the tool definition, they are silently ignored (and logged in the tool-call audit).

See [Agentic Mode](08-Agentic-Mode) for the full agentic review workflow.

## Environment Passthrough

Custom command subprocesses use an **allowlist** model: only a minimal set of safe system variables is inherited by default (`PATH`, `HOME`, `LANG`, `SHELL`, `TERM`, `USER`, locale prefixes like `LC_*`, and XDG directories like `XDG_*`). Everything else — API keys, tokens, database credentials, CI secrets — is stripped.

If your custom tools need additional env vars, declare them in the `environment` field:

```markdown
---
name: infra-reviewer
description: Reviews infrastructure changes
environment:
  - JIRA_TOKEN
  - AWS_*          # prefix glob — matches AWS_REGION, AWS_SECRET_ACCESS_KEY, etc.
  - DOCKER_HOST
tools:
  - name: deploy_check
    description: Check deployment status
    command: curl -sH "Authorization: Bearer $JIRA_TOKEN" https://jira.example.com/status
---
```

**Default safe variables:** `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `LANG`, `HOSTNAME`, `PWD`, `TMPDIR`, `TEMP`, `TMP`, `SHLVL`, `COLORTERM`, `TERM_PROGRAM`, plus any variable starting with `LC_` or `XDG_`.

All other env vars are stripped unless explicitly listed in `environment`. Exact names and prefix globs (ending with `*`) are supported.

## Related Pages

- [Reviewer Profiles](06-Reviewer-Profiles) — built-in profiles and selection
- [Agentic Mode](08-Agentic-Mode) — using tools during reviews
- [Project Documentation](12-Project-Docs) — teaching the reviewer your conventions via `REVIEW.md`
- [Configuration](16-Configuration) — `--profile-dir` and related settings
