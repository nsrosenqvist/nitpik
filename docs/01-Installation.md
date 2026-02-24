# Installation

nitpik is a single binary with no runtime dependencies. Install it once and you're ready to review.

---

## Install Script (Recommended)

The install script detects your platform, downloads the latest release, verifies the SHA256 checksum, and installs the binary:

```bash
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/nitpik/main/install.sh | bash
```

**Options:**

```bash
# Install to a custom directory (default: /usr/local/bin)
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/nitpik/main/install.sh | bash -s -- --dir ~/.local/bin

# Install a specific version
curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/nitpik/main/install.sh | bash -s -- --version v0.3.0
```

You can also set `NITPIK_INSTALL_DIR` and `NITPIK_VERSION` as environment variables.

Verify the installation:

```bash
nitpik --version
```

## Pre-built Binary (Manual)

Alternatively, download a release archive directly from the [GitHub Releases page](https://github.com/nsrosenqvist/nitpik/releases/latest) and place it on your `PATH`.

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
