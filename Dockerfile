# syntax=docker/dockerfile:1

# ---- Base toolchain with cargo-chef ----
FROM rust:1-bookworm AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked

# ---- Plan: capture the dependency graph (busts only when deps change) ----
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

# ---- Stage 1: build the WASM client (Leptos CSR via Trunk) ----
FROM chef AS client
RUN rustup target add wasm32-unknown-unknown \
 && cargo install trunk --locked
COPY --from=planner /app/recipe.json recipe.json
# Cook only the client's deps for wasm; the shared registry + per-stage target
# cache mounts persist compiled artifacts across builds (incremental rebuilds).
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target-client,target=/app/target \
    cargo chef cook --release --target wasm32-unknown-unknown -p client --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Both bake into the wasm at compile time (option_env!); set them here if your
# public URL differs from the browser origin or you want a custom STUN server.
ARG PUBLIC_ORIGIN
ARG STUN_URL
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target-client,target=/app/target \
    cd crates/client \
 && if [ -n "$PUBLIC_ORIGIN" ]; then export PUBLIC_ORIGIN; fi \
 && if [ -n "$STUN_URL" ]; then export STUN_URL; fi \
 && trunk build --release
# trunk writes to /app/crates/client/dist (a real layer, not the target mount)

# ---- Stage 2: build the server as a static musl binary ----
FROM chef AS server
RUN rustup target add x86_64-unknown-linux-musl \
 && apt-get update && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target-server,target=/app/target \
    cargo chef cook --release --target x86_64-unknown-linux-musl -p server --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# target/ is a cache mount (not part of the image), so copy the binary out to a
# normal path before the stage ends.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=cargo-target-server,target=/app/target \
    cargo build --release -p server --target x86_64-unknown-linux-musl \
 && cp /app/target/x86_64-unknown-linux-musl/release/server /app/server

# ---- Stage 3: minimal runtime ----
FROM scratch
COPY --from=server /app/server /server
COPY --from=client /app/crates/client/dist /dist
ENV CLIENT_DIST=/dist
ENV BIND_ADDR=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["/server"]
