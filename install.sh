#!/usr/bin/env bash
# install.sh — Install the latest nitpik release for the current platform.
#
# Usage:
#   curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/nitpik/main/install.sh | sh
#   curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/nitpik/main/install.sh | sh -s -- --dir ~/.local/bin
#   curl -sSfL https://raw.githubusercontent.com/nsrosenqvist/nitpik/main/install.sh | sh -s -- --version v0.3.0
#
# Environment variables:
#   NITPIK_INSTALL_DIR  — Override the default install directory (default: /usr/local/bin)
#   NITPIK_VERSION      — Install a specific version tag (default: latest)

set -eu

REPO="nsrosenqvist/nitpik"
BINARY="nitpik"
DEFAULT_INSTALL_DIR="/usr/local/bin"

# ── Helpers ──────────────────────────────────────────────────────────

say() {
    printf '%s\n' "$*"
}

err() {
    say "Error: $*" >&2
    exit 1
}

need() {
    if ! command -v "$1" > /dev/null 2>&1; then
        err "need '$1' (command not found)"
    fi
}

# ── Parse arguments ──────────────────────────────────────────────────

INSTALL_DIR="${NITPIK_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
VERSION="${NITPIK_VERSION:-latest}"

while [ $# -gt 0 ]; do
    case "$1" in
        --dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
        --help)
            say "Usage: install.sh [--dir DIR] [--version TAG]"
            say ""
            say "Options:"
            say "  --dir DIR        Install directory (default: $DEFAULT_INSTALL_DIR)"
            say "  --version TAG    Version tag to install, e.g. v0.3.0 (default: latest)"
            say ""
            say "Environment variables:"
            say "  NITPIK_INSTALL_DIR   Override install directory"
            say "  NITPIK_VERSION       Override version tag"
            exit 0
            ;;
        *)
            err "unknown option: $1 (use --help for usage)"
            ;;
    esac
done

# ── Detect platform ─────────────────────────────────────────────────

detect_target() {
    local os arch target

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os="unknown-linux-gnu" ;;
        Darwin) os="apple-darwin" ;;
        *)      err "unsupported OS: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)   arch="aarch64" ;;
        *)               err "unsupported architecture: $arch" ;;
    esac

    target="${arch}-${os}"
    say "$target"
}

# ── Resolve version ─────────────────────────────────────────────────

resolve_version() {
    if [ "$VERSION" = "latest" ]; then
        # Use the GitHub API redirect to resolve the latest tag.
        local url="https://api.github.com/repos/${REPO}/releases/latest"
        local tag

        tag="$(curl -sSfL "$url" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/')"

        if [ -z "$tag" ]; then
            err "could not determine latest release from GitHub API"
        fi

        say "$tag"
    else
        # Ensure the tag starts with 'v'.
        case "$VERSION" in
            v*) say "$VERSION" ;;
            *)  say "v${VERSION}" ;;
        esac
    fi
}

# ── Main ─────────────────────────────────────────────────────────────

main() {
    need curl
    need tar
    need uname

    say "Detecting platform..."
    local target
    target="$(detect_target)"
    say "  Platform: ${target}"

    say "Resolving version..."
    local tag
    tag="$(resolve_version)"
    say "  Version:  ${tag}"

    local archive_url checksum_url archive_name
    archive_name="${BINARY}-${target}.tar.gz"
    archive_url="https://github.com/${REPO}/releases/download/${tag}/${archive_name}"
    checksum_url="https://github.com/${REPO}/releases/download/${tag}/SHA256SUMS"

    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    say "Downloading ${archive_name}..."
    curl -sSfL -o "${tmpdir}/${archive_name}" "$archive_url" \
        || err "download failed — does ${tag} have a release for ${target}?"

    # Verify checksum if sha256sum or shasum is available.
    if command -v sha256sum > /dev/null 2>&1; then
        say "Verifying checksum (sha256sum)..."
        curl -sSfL -o "${tmpdir}/SHA256SUMS" "$checksum_url" \
            || err "could not download SHA256SUMS"
        (cd "$tmpdir" && grep "$archive_name" SHA256SUMS | sha256sum -c --quiet -) \
            || err "checksum verification failed"
    elif command -v shasum > /dev/null 2>&1; then
        say "Verifying checksum (shasum)..."
        curl -sSfL -o "${tmpdir}/SHA256SUMS" "$checksum_url" \
            || err "could not download SHA256SUMS"
        (cd "$tmpdir" && grep "$archive_name" SHA256SUMS | shasum -a 256 -c --quiet -) \
            || err "checksum verification failed"
    else
        say "  (skipping checksum verification — neither sha256sum nor shasum found)"
    fi

    say "Extracting..."
    tar xzf "${tmpdir}/${archive_name}" -C "$tmpdir"

    # Install the binary.
    if [ -w "$INSTALL_DIR" ]; then
        install -m 755 "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
    else
        say "  ${INSTALL_DIR} is not writable — using sudo"
        sudo install -m 755 "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
    fi

    say ""
    say "✓ ${BINARY} ${tag} installed to ${INSTALL_DIR}/${BINARY}"

    # Verify the installed binary runs.
    if command -v "$BINARY" > /dev/null 2>&1; then
        say "  $("$BINARY" --version 2>/dev/null || true)"
    else
        say ""
        say "  Note: ${INSTALL_DIR} may not be in your PATH."
        say "  Add it with: export PATH=\"${INSTALL_DIR}:\$PATH\""
    fi
}

main
