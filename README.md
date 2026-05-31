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
- `PUBLIC_ORIGIN` (client, compile-time) — origin used to build share
  links/QR, e.g. `https://files.example.com`. Defaults to the browser's
  runtime origin (`window.location.origin`); set this only when the public URL
  differs from what the browser sees (e.g. behind a reverse proxy).

## Room codes & rate limiting
Room codes are 6 lowercase, unambiguous characters (`a–z` + `2–9`, no
`i/l/o/0/1`) — easy to read and type. To keep the short codes from being
enumerable, the server rate-limits failed join attempts per source IP (30 per
60s); a peer over the limit is told the room doesn't exist. Behind a reverse
proxy the limit keys on the proxy's address (the `X-Forwarded-For` header is
not trusted), so per-IP limiting is most effective on a directly-exposed
server.

## Known limitation
True P2P with no TURN relay. Peers behind symmetric NAT may fail to connect
directly; the UI surfaces a clear error. A TURN fallback is a future
enhancement.
