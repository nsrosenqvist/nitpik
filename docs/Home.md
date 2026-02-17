# nitpik Documentation

AI-powered code reviews for your team. Bring your own model, bring your own API key.

---

## Getting Started

New to nitpik? Start here:

1. **[Installation](01-Installation)** — download the binary, install from source, or pull the Docker image.
2. **[Quick Start](02-Quick-Start)** — run your first review in under two minutes.
3. **[LLM Providers](03-Providers)** — connect Anthropic, OpenAI, Gemini, or any compatible API.

## Using nitpik

- **[Diff Inputs](04-Diff-Inputs)** — git diffs, file scans, patches, and stdin.
- **[Reviewer Profiles](05-Reviewer-Profiles)** — built-in specialist reviewers and how to combine them.
- **[Custom Profiles](06-Custom-Profiles)** — write your own reviewer with Markdown and YAML.
- **[Agentic Mode](07-Agentic-Mode)** — let the LLM explore your codebase with tools.
- **[Output Formats](08-Output-Formats)** — terminal, JSON, GitHub, GitLab, Bitbucket, and Forgejo.

## How It Works

- **[How Reviews Work](09-How-Reviews-Work)** — context assembly, multi-agent coordination, and quality post-processing.
- **[Caching & Prior Findings](10-Caching)** — content-hash caching and iterative review continuity.
- **[Secret Scanning](11-Secret-Scanning)** — detect and redact secrets before code reaches the LLM.
- **[Project Documentation Context](12-Project-Docs)** — teach the reviewer your team's conventions.

## Deployment

- **[Configuration Reference](13-Configuration)** — `.nitpik.toml`, environment variables, and CLI flags.
- **[CI/CD Integration](14-CI-Integration)** — GitHub Actions, GitLab CI, Bitbucket Pipelines, Woodpecker/Forgejo.

## Reference

- **[CLI Reference](15-CLI-Reference)** — every command and flag.
- **[Troubleshooting](16-Troubleshooting)** — common issues and solutions.
- **[Licensing](17-Licensing)** — free tier, commercial activation, and license management.

---

[Website](https://nitpik.dev) · [GitHub](https://github.com/nsrosenqvist/nitpik) · [Report an Issue](https://github.com/nsrosenqvist/nitpik/issues)
