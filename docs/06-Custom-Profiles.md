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
| `always_include` | No | When `true`, the profile is added to every `auto` review regardless of file heuristics. Defaults to `false`. See [Always-On Profiles](#always-on-profiles). |
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
nitpik review --diff-base main --profile-dir ./agents --profile backend,team-conventions
```

## Overriding Built-In Profiles

A custom profile whose `name` matches a built-in (`backend`, `frontend`, `architect`, `security`, `general`) replaces the built-in when `--profile-dir` is set. This lets you tune the shipped profiles to your team's needs without forking the project.

For example, drop a file at `./agents/backend.md`:

```markdown
---
name: backend
description: Backend review tuned for our Rust services
tags: [backend, rust, performance]
---

You are reviewing backend Rust code for our team...
```

Then run:

```bash
nitpik review --diff-base main --profile-dir ./agents --profile backend
```

nitpik loads your `backend.md` instead of the built-in. The override applies everywhere the profile is referenced — `--profile backend`, `--tag backend`, auto-selection, and `nitpik profiles` all use your version.

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

Some reviewers should run on every change, regardless of which files were touched — security is the obvious example, but teams often want similar coverage for documentation drift, license headers, telemetry conventions, or other cross-cutting concerns.

Set `always_include: true` in a profile's frontmatter to make it part of every `auto` review:

```markdown
---
name: docs-drift
description: Flags changes that may invalidate existing documentation
tags: [docs, drift]
always_include: true
---

You are a documentation accuracy reviewer.

When a code change modifies a public API, configuration option, CLI flag, or
documented behavior, check whether existing documentation still matches:

1. **Scan related docs.** If the change is in `src/api/`, search `docs/api/`,
   `README.md`, and any inline rustdoc for references to the changed symbol.
2. **Flag drift, don't rewrite it.** Report stale paragraphs as findings —
   tell the author which doc section is now inaccurate and what changed.
   Don't suggest the new wording; that's a separate task.
3. **Severity guidance.**
   - `error` — public API contract changed and docs still describe the old behavior.
   - `warning` — internal behavior changed in a way users would notice (defaults, error messages, output format).
   - `info` — minor changes that may warrant a doc refresh but won't mislead users.

Don't report on changes to test files, internal helpers, or undocumented code.
```

Use it like any other profile — drop the file in your `--profile-dir` and `auto` picks it up automatically:

```bash
nitpik review --diff-base main --profile-dir ./agents --profile auto
```

> **Note:** `always_include` only applies to `auto` mode. Explicit `--profile` and `--tag` selections stay literal — if you list profiles by name, only those run.

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

See [Agentic Mode](07-Agentic-Mode) for the full agentic review workflow.

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

- [Reviewer Profiles](05-Reviewer-Profiles) — built-in profiles and selection
- [Agentic Mode](07-Agentic-Mode) — using tools during reviews
- [Project Documentation](13-Project-Docs) — teaching the reviewer your conventions via `REVIEW.md`
- [Configuration](14-Configuration) — `--profile-dir` and related settings
