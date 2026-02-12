# Multi-stage Dockerfile for nitpik
# Produces a minimal static binary image (~30MB)

# Stage 1: Build
FROM rust:1.89-bookworm AS builder

WORKDIR /build

# Cache dependencies by building a dummy project first
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Copy source and build for real
COPY src/ src/

RUN cargo build --release --bin nitpik && \
    strip target/release/nitpik

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        git \
        ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/nitpik /usr/local/bin/nitpik

# Default working directory (mount your repo here)
WORKDIR /repo

ENTRYPOINT ["nitpik"]
CMD ["review", "--help"]
