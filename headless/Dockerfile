# nodalmerge-headless — peer worker image for Kubernetes / pod workloads.
#
# Build from repo root:
#   docker build -f headless/Dockerfile -t nodalmerge-headless .
#
# Run:
#   docker run --rm \
#     -e NODALMERGE_HEADLESS_SERVER_URL=ws://host.docker.internal:8080/ws/my-room \
#     -e NODALMERGE_HEADLESS_ROOM=my-room \
#     -e NODALMERGE_HEADLESS_BACKEND=file \
#     -v nodalmerge-peer-data:/data \
#     nodalmerge-headless

FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p nodalmerge-headless

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/nodalmerge-headless /usr/local/bin/nodalmerge-headless

ENV NODALMERGE_HEADLESS_BACKEND=file
ENV NODALMERGE_HEADLESS_DATA_DIR=/data
ENV NODALMERGE_HEADLESS_RUN_SECS=30
ENV NODALMERGE_HEADLESS_NEGOTIATE_IBF=1
ENV NODALMERGE_HEADLESS_NEGOTIATE_MST=1

VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/nodalmerge-headless"]
