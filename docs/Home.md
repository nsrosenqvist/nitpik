# nitpik Documentation

AI-powered code reviews for your team. Bring your own model, bring your own API key.

---

## Getting Started

New to nitpik? Start here:

1. **[Installation](01-Installation)** — download the binary, install from source, or pull the Docker image.
2. **[Quick Start](02-Quick-Start)** — run your first review in under two minutes.
3. **[LLM Providers](03-Providers)** — connect Anthropic, OpenAI, Gemini, or any compatible API.
4. **[GitHub Models](04-GitHub-Models)** — run nitpik on free GitHub-hosted models with `GITHUB_TOKEN`, ideal for CI.

## Using nitpik

- **[Diff Inputs](05-Diff-Inputs)** — git diffs, file scans, patches, and stdin.
- **[Reviewer Profiles](06-Reviewer-Profiles)** — built-in specialist reviewers and how to combine them.
- **[Custom Profiles](07-Custom-Profiles)** — write your own reviewer with Markdown and YAML.
- **[Agentic Mode](08-Agentic-Mode)** — let the LLM explore your codebase with tools.
- **[Output Formats](09-Output-Formats)** — terminal, JSON, GitHub, GitLab, Bitbucket, and Forgejo.

## How It Works

- **[How Reviews Work](10-How-Reviews-Work)** — context assembly, multi-agent coordination, and quality post-processing.
- **[Caching & Prior Findings](11-Caching)** — content-hash caching and iterative review continuity.
- **[Project Documentation Context](12-Project-Docs)** — teach the reviewer your team's conventions.

## Security & Privacy

- **[Security Model](13-Security-Model)** — what nitpik sends to LLM providers, subprocess sandboxing, telemetry, and on-disk data.
- **[Secret Scanning](14-Secret-Scanning)** — detect and redact secrets before code reaches the LLM.
- **[Threat Scanning](15-Threat-Scanning)** — detect obfuscated payloads, dangerous APIs, supply chain attacks, and homoglyph tricks.

## Deployment

- **[Configuration Reference](16-Configuration)** — `.nitpik.toml`, environment variables, and CLI flags.
- **[CI/CD Integration](17-CI-Integration)** — GitHub Actions, GitLab CI, Bitbucket Pipelines, Woodpecker/Forgejo.

## Reference

- **[CLI Reference](18-CLI-Reference)** — every command and flag.
- **[Troubleshooting](19-Troubleshooting)** — common issues and solutions.
- **[Licensing](20-Licensing)** — free tier, commercial activation, and license management.

---

[Website](https://nitpik.dev) · [GitHub](https://github.com/nsrosenqvist/nitpik) · [Report an Issue](https://github.com/nsrosenqvist/nitpik/issues)
