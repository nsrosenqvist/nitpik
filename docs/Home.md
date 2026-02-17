# nitpik Documentation

AI-powered code reviews for your team. Bring your own model, bring your own API key.

---

## Getting Started

New to nitpik? Start here:

1. **[Installation](Installation)** — download the binary, install from source, or pull the Docker image.
2. **[Quick Start](Quick-Start)** — run your first review in under two minutes.
3. **[LLM Providers](Providers)** — connect Anthropic, OpenAI, Gemini, or any compatible API.

## Using nitpik

- **[Diff Inputs](Diff-Inputs)** — git diffs, file scans, patches, and stdin.
- **[Reviewer Profiles](Reviewer-Profiles)** — built-in specialist reviewers and how to combine them.
- **[Custom Profiles](Custom-Profiles)** — write your own reviewer with Markdown and YAML.
- **[Agentic Mode](Agentic-Mode)** — let the LLM explore your codebase with tools.
- **[Output Formats](Output-Formats)** — terminal, JSON, GitHub, GitLab, Bitbucket, and Forgejo.

## How It Works

- **[How Reviews Work](How-Reviews-Work)** — context assembly, multi-agent coordination, and quality post-processing.
- **[Caching & Prior Findings](Caching)** — content-hash caching and iterative review continuity.
- **[Secret Scanning](Secret-Scanning)** — detect and redact secrets before code reaches the LLM.
- **[Project Documentation Context](Project-Docs)** — teach the reviewer your team's conventions.

## Deployment

- **[Configuration Reference](Configuration)** — `.nitpik.toml`, environment variables, and CLI flags.
- **[CI/CD Integration](CI-Integration)** — GitHub Actions, GitLab CI, Bitbucket Pipelines, Woodpecker/Forgejo.

## Reference

- **[CLI Reference](CLI-Reference)** — every command and flag.
- **[Troubleshooting](Troubleshooting)** — common issues and solutions.
- **[Licensing](Licensing)** — free tier, commercial activation, and license management.

---

[Website](https://nitpik.dev) · [GitHub](https://github.com/nsrosenqvist/nitpik) · [Report an Issue](https://github.com/nsrosenqvist/nitpik/issues)
