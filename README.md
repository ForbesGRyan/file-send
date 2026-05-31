# file-send

Anonymous peer-to-peer file transfer. Two browsers connect directly over
WebRTC; a Rust signaling server only brokers the handshake — file bytes never
pass through it.

## Architecture
- `crates/shared` — JSON signaling wire types.
- `crates/server` — Axum signaling server; also serves the built client.
- `crates/client` — Leptos CSR (WASM) app; all WebRTC, chunking, and UI.

## Prerequisites
- Rust (edition 2024), `cargo`
- [`trunk`](https://trunkrs.dev): `cargo install trunk`
- wasm target: `rustup target add wasm32-unknown-unknown`

## Develop
Two terminals:
```bash
# 1. backend (signaling + static)
cargo run -p server

# 2. client with hot-reload + /ws proxy to backend
cd crates/client && trunk serve
```
Open the Trunk dev URL (http://localhost:8080). The `/ws` proxy forwards to the
backend on :3000.

## Production build
```bash
cd crates/client && trunk build --release && cd ../..
cargo run --release -p server     # serves crates/client/dist on :3000
```

## Configuration
- `BIND_ADDR` (server) — listen address, default `0.0.0.0:3000`.
- `CLIENT_DIST` (server) — static dir, default `crates/client/dist`.
- `STUN_URL` (client, compile-time) — STUN server, default
  `stun:stun.l.google.com:19302`.

## Known limitation
True P2P with no TURN relay. Peers behind symmetric NAT may fail to connect
directly; the UI surfaces a clear error. A TURN fallback is a future
enhancement.
