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

# Copy only manifests first for better layer caching. Any `path = "..."`
# workspace member needs its Cargo.toml present for `cargo fetch` to resolve.
COPY Cargo.toml Cargo.lock* ./
COPY core/Cargo.toml                       core/Cargo.toml
COPY gc/Cargo.toml                         gc/Cargo.toml
COPY host-core/Cargo.toml                  host-core/Cargo.toml
COPY host-axum/Cargo.toml                  host-axum/Cargo.toml
COPY host-ffi/Cargo.toml                   host-ffi/Cargo.toml
COPY bridge/Cargo.toml                     bridge/Cargo.toml
COPY server/Cargo.toml                     server/Cargo.toml
COPY jwt-bridge/Cargo.toml                 jwt-bridge/Cargo.toml
COPY s3-blobs/Cargo.toml                   s3-blobs/Cargo.toml
COPY node-stores/conformance/Cargo.toml    node-stores/conformance/Cargo.toml
COPY node-stores/mongo/Cargo.toml          node-stores/mongo/Cargo.toml
COPY dev-server/Cargo.toml                 dev-server/Cargo.toml
COPY node-stores/postgres/Cargo.toml       node-stores/postgres/Cargo.toml

# Stub source trees so cargo can resolve + fetch without real code.
# Core's Cargo.toml declares several `[[bench]]` targets; cargo validates
# their files exist at manifest-parse time, so we stub them too. Same for
# any integration-test targets we add later.
RUN mkdir -p core/src core/benches \
             gc/src \
             host-core/src \
             host-axum/src \
             host-ffi/src host-ffi/benches \
             bridge/src \
             server/src server/src/bin server/tests \
             jwt-bridge/src \
             s3-blobs/src \
             node-stores/conformance/src \
             node-stores/mongo/src \
             dev-server/src \
             node-stores/postgres/src \
    && echo 'fn main(){}' > server/src/main.rs \
    && echo 'fn main(){}' > server/src/bin/authz_conformance_runner.rs \
    && echo 'fn main(){}' > dev-server/src/main.rs \
    && echo ''            > core/src/lib.rs \
    && echo ''            > gc/src/lib.rs \
    && echo ''            > host-core/src/lib.rs \
    && echo ''            > host-axum/src/lib.rs \
    && echo ''            > host-ffi/src/lib.rs \
    && echo 'fn main(){}' > host-ffi/benches/host_runtime_vs_ffi.rs \
    && echo ''            > bridge/src/lib.rs \
    && echo ''            > jwt-bridge/src/lib.rs \
    && echo ''            > s3-blobs/src/lib.rs \
    && echo ''            > node-stores/conformance/src/lib.rs \
    && echo ''            > node-stores/mongo/src/lib.rs \
    && echo ''            > node-stores/postgres/src/lib.rs \
    && for b in merge_10k sync_handshake resolve_1k blob_verify tx_hash text_trace_rustcode text_write_path; do \
           echo 'fn main(){}' > "core/benches/$b.rs"; \
       done \
    && cargo fetch --locked || cargo fetch

# Now bring in the real sources and build release.
COPY . .
RUN cargo build --release -p nodalmerge-server

# ─── Runtime ───────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for the server.
RUN useradd --system --home /data --shell /usr/sbin/nologin nodalmerge \
    && mkdir -p /data \
    && chown nodalmerge:nodalmerge /data

COPY --from=builder /src/target/release/nodalmerge-server /usr/local/bin/nodalmerge-server

USER nodalmerge
WORKDIR /data
VOLUME ["/data"]
EXPOSE 7878

ENV RUST_LOG=info
ENTRYPOINT ["nodalmerge-server"]
CMD ["--store", "/data"]
