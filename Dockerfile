# syntax=docker/dockerfile:1

# ---- Stage 1: build the WASM client (Leptos CSR via Trunk) ----
FROM rust:1-bookworm AS client
WORKDIR /app
RUN rustup target add wasm32-unknown-unknown \
 && cargo install trunk --locked
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Both bake into the wasm at compile time (option_env!); set them here if your
# public URL differs from the browser origin or you want a custom STUN server.
ARG PUBLIC_ORIGIN
ARG STUN_URL
RUN cd crates/client \
 && if [ -n "$PUBLIC_ORIGIN" ]; then export PUBLIC_ORIGIN; fi \
 && if [ -n "$STUN_URL" ]; then export STUN_URL; fi \
 && trunk build --release
# -> /app/crates/client/dist

# ---- Stage 2: build the server as a static musl binary ----
FROM rust:1-bookworm AS server
WORKDIR /app
RUN rustup target add x86_64-unknown-linux-musl \
 && apt-get update && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p server --target x86_64-unknown-linux-musl
# -> /app/target/x86_64-unknown-linux-musl/release/server

# ---- Stage 3: minimal runtime ----
FROM scratch
COPY --from=server /app/target/x86_64-unknown-linux-musl/release/server /server
COPY --from=client /app/crates/client/dist /dist
ENV CLIENT_DIST=/dist
ENV BIND_ADDR=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["/server"]
