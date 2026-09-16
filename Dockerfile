# syntax=docker/dockerfile:1.7
# Multi-stage build for Mediagenerator.
#
# Stage 1 (chef/planner/builder) uses cargo-chef so that a change to the source
# does not invalidate the dependency layer. Stage 2 is a debian-slim runtime
# with a non-root user; distroless is not used because the runtime needs the
# CA bundle and a shell-less healthcheck is harder to express there.

ARG RUST_VERSION=1.90
ARG DEBIAN_RELEASE=bookworm

# --- planner --------------------------------------------------------------
FROM rust:${RUST_VERSION}-slim-${DEBIAN_RELEASE} AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# --- builder --------------------------------------------------------------
FROM chef AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --bin mediagenerator

# --- runtime --------------------------------------------------------------
FROM debian:${DEBIAN_RELEASE}-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 app \
    && useradd --system --uid 10001 --gid app --no-create-home app

WORKDIR /app
COPY --from=builder /app/target/release/mediagenerator /usr/local/bin/mediagenerator
COPY --chown=root:root assets ./assets
COPY --chown=root:root migrations ./migrations

USER app:app
ENV APP_PORT=8080 \
    APP_ENV=production \
    ASSETS_DIR=/app/assets
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/usr/local/bin/mediagenerator"]
