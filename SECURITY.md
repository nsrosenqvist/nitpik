# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| latest  | :white_check_mark: |

## Reporting a Vulnerability

Please **do not** open a public issue for security vulnerabilities.

Instead, report vulnerabilities by either:

- Using [GitHub Private Vulnerability Reporting](https://github.com/nsrosenqvist/nitpik/security/advisories/new)
- Emailing **security@nitpik.dev**

Include as much detail as possible: steps to reproduce, affected versions, and potential impact.

## Response Timeline

- **Acknowledgment**: within 48 hours
- **Initial assessment**: within 5 business days
- **Fix for critical issues**: within 7 days where feasible

## Scope

- The `nitpik` CLI binary and its published Docker image
- Built-in agent profiles and agentic tool implementations
- Secret scanning and environment sanitization logic

Out of scope: third-party LLM provider APIs, user-authored custom profiles, and infrastructure not maintained by the nitpik project.
