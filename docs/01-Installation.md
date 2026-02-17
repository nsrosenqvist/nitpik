# Installation

nitpik is a single binary with no runtime dependencies. Install it once and you're ready to review.

---

## Pre-built Binary (Recommended)

Download the latest release for your platform from the [GitHub Releases page](https://github.com/nsrosenqvist/nitpik/releases/latest) and place it somewhere on your `PATH`.

**Linux (x86_64):**

```bash
curl -sSfL https://github.com/nsrosenqvist/nitpik/releases/latest/download/nitpik-x86_64-unknown-linux-gnu.tar.gz | sudo tar xz -C /usr/local/bin
```

**macOS (Apple Silicon):**

```bash
curl -sSfL https://github.com/nsrosenqvist/nitpik/releases/latest/download/nitpik-aarch64-apple-darwin.tar.gz | tar xz -C /usr/local/bin
```

**macOS (Intel):**

```bash
curl -sSfL https://github.com/nsrosenqvist/nitpik/releases/latest/download/nitpik-x86_64-apple-darwin.tar.gz | tar xz -C /usr/local/bin
```

Verify the installation:

```bash
nitpik --version
```

## Build from Source

If you have a Rust toolchain installed:

```bash
cargo install --path .
```

This compiles an optimized release binary and places it in `~/.cargo/bin/`.

## Docker

The official Docker image ships with `git` and the `nitpik` binary, ready for CI pipelines:

```bash
docker pull ghcr.io/nsrosenqvist/nitpik:latest
```

Run a review by mounting your repository:

```bash
docker run --rm \
  -v "$(pwd)":/repo \
  -e NITPIK_PROVIDER=anthropic \
  -e ANTHROPIC_API_KEY \
  ghcr.io/nsrosenqvist/nitpik:latest review --diff-base main
```

### Docker vs Native

| | Native binary | Docker |
|---|---|---|
| **Startup** | Instant | Container overhead (~1-2s) |
| **Self-update** | `nitpik update` | Rebuild/re-pull the image |
| **CI isolation** | Runs in host environment | Fully isolated |
| **Best for** | Local development, simple CI | CI pipelines, reproducible environments |

> **Tip:** In CI, prefer Docker for isolation and reproducibility. For local development, the native binary is faster and supports `nitpik update`.

## Self-Update

nitpik can update itself to the latest GitHub release:

```bash
nitpik update             # update to latest (skips if already current)
nitpik update --force     # re-download even if on latest
```

The update downloads the release archive for your platform, verifies its SHA256 checksum, and atomically replaces the running binary.

If installed in a system directory (e.g. `/usr/local/bin`), you may need `sudo`:

```bash
sudo nitpik update
```

> **Note:** In Docker containers and CI, nitpik warns you to rebuild the image or pin a version instead of self-updating.

## System Requirements

- **Git** — required for `--diff-base` mode. Not needed for `--scan`, `--diff-file`, or `--diff-stdin`.
- **Network access** — nitpik calls your configured LLM provider's API. No other network access is required (telemetry is optional and can be disabled).

## Related Pages

- [Quick Start](02-Quick-Start) — run your first review
- [LLM Providers](03-Providers) — connect an API key
- [CI/CD Integration](14-CI-Integration) — set up nitpik in your pipeline
