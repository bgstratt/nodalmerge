# syntax=docker/dockerfile:1.6
#
# Reference Dockerfile for self-hosting `nodalmerge-server`.
#
# Build:   docker build -t nodalmerge-server .
# Run:     docker run --rm -p 7878:7878 -v $PWD/data:/data \
#              -e RUST_LOG=info nodalmerge-server
#
# The server persists to /data (mount a host dir or named volume for
# durability). SQLite is bundled statically — no system libs required at
# runtime beyond libc + ca-certificates.

# ─── Builder ───────────────────────────────────────────────────────────────
# Rust 1.85+ required: some transitive deps (e.g. `time-core`) have adopted
# `edition = "2024"`, which was stabilized in 1.85. Pinning to a recent
# stable keeps the build reproducible without chasing nightly.
FROM rust:1.90-slim-bookworm AS builder

WORKDIR /src

# Bring in the full workspace. The previous manifest-only staging step
# enumerated workspace members by hand for layer caching, but kept drifting
# out of sync as members were added (wrapper crates, runtime-local, headless,
# cli, ...), breaking `cargo fetch` resolution. BuildKit cache mounts on the
# cargo registry and target dir give dependency/incremental-build caching
# without needing the member list duplicated here.
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p nodalmerge-server \
    && cp target/release/nodalmerge-server /tmp/nodalmerge-server

# ─── Runtime ───────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for the server.
RUN useradd --system --home /data --shell /usr/sbin/nologin nodalmerge \
    && mkdir -p /data \
    && chown nodalmerge:nodalmerge /data

COPY --from=builder /tmp/nodalmerge-server /usr/local/bin/nodalmerge-server

USER nodalmerge
WORKDIR /data
VOLUME ["/data"]
EXPOSE 7878

ENV RUST_LOG=info
ENTRYPOINT ["nodalmerge-server"]
CMD ["--store", "/data"]
