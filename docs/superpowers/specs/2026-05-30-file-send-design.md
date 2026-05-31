# file-send — Anonymous P2P File Transfer

**Date:** 2026-05-30
**Status:** Approved design

## Summary

A web app that lets two anonymous clients establish a true peer-to-peer
connection and transfer files directly between their browsers. A Rust server
performs **signaling only** — it brokers the WebRTC handshake but never sees
file bytes. The frontend is a Leptos client-side-rendered (CSR) WASM app.

## Goals

- Two anonymous clients (no accounts) transfer files directly browser-to-browser.
- Pairing via a shareable room link.
- Drag-and-drop multiple files with per-file progress.
- Deployable to a real host (configurable STUN, chunked transfers for large
  files, clean reconnection/disconnect handling).

## Non-Goals (v1)

- TURN relay / NAT-traversal fallback. True P2P only; symmetric-NAT failures are
  surfaced as a clear error and documented as a future add-on.
- Accounts, history, persistence. Rooms are ephemeral and in-memory.
- Server-side rendering / SEO. The app is a pure P2P client.

## Architecture

Cargo workspace, three crates:

```
file-send/
├── crates/
│   ├── shared/   # signaling message types (serde), shared server <-> client
│   ├── server/   # Axum: serves static client + /ws signaling (native binary)
│   └── client/   # Leptos CSR app, WebRTC logic (wasm, built with Trunk)
```

### server (native)

- Axum + Tokio. Serves the built client static files and hosts `GET /ws`
  (WebSocket signaling).
- In-memory room registry: `Mutex<HashMap<RoomId, Room>>` (or `dashmap`). No
  database, no persistence.
- A `Room` holds up to two peer connections. The server relays signaling
  messages between them and enforces the two-peer limit.
- Empty rooms are removed from the registry when both peers leave.

### client (wasm, Leptos CSR, built with Trunk)

- All WebRTC lives here (browser-only APIs via `web-sys`:
  `RtcPeerConnection`, `RtcDataChannel`, `File`, `Blob`, `FileReader`).
- UI: create/join room, drag-drop file selection, per-file progress bars,
  connection status.
- Owns chunking, backpressure, reassembly, and download triggering.

### shared

- `SignalMsg` enum serialized as JSON. Variants cover: offer, answer, ICE
  candidate, peer-joined, peer-left, room-full, room-not-found.
- Both crates depend on it so the wire format cannot drift.

## Connection Flow

1. A opens the app, clicks **Create room**. WS connects; server mints a
   `room_id` and returns it. A sees a shareable link `/#/room/<id>`.
2. A shares the link out-of-band. B opens it; WS connects with `room_id`.
   Server attaches B as the second peer and notifies A with `PeerJoined`.
   A third joiner receives `RoomFull` and is rejected.
3. **WebRTC handshake** (server relays only): A (initiator) creates an
   `RtcPeerConnection` and a data channel, makes an SDP offer → server relays
   to B → B sets remote description, creates an answer → relayed back to A.
   ICE candidates are relayed both ways. A configurable STUN server (defaults
   to public Google STUN, e.g. `stun:stun.l.google.com:19302`) is used for
   candidate gathering.
4. Data channel opens → signaling complete. File bytes flow **directly
   browser-to-browser** and never pass through the server. WebRTC data
   channels are DTLS-encrypted by default.

### Known Limitation

True P2P with no TURN: peers behind symmetric NAT may fail to connect
directly. The client surfaces a clear "couldn't establish direct connection"
error with a retry option. A TURN relay is a documented future enhancement.

## Data-Channel File Protocol

One file is in flight at a time per direction. Multiple selected files queue
and send sequentially (guarantees ordering, avoids interleaving complexity).

The data channel runs in **ordered + reliable** mode. Framing mixes JSON
control messages (sent as strings) and binary chunks (sent as `ArrayBuffer`):

- `FileStart { id, name, size, mime }` — JSON string; announces the next file.
- **N binary chunks** — raw `ArrayBuffer`, ~16 KB each (safe data-channel
  message size). Order guaranteed by the channel.
- `FileEnd { id }` — JSON string; receiver finalizes the file.

The receiver distinguishes by message type: a `string` is a control message,
an `ArrayBuffer` is a chunk for the currently-open file. It reassembles chunks
into a `Blob`, then triggers a browser download via an object URL.

### Backpressure

Before sending each chunk, the sender checks `dataChannel.bufferedAmount`. If
it exceeds a high-water mark (e.g. 1 MB), the sender pauses and resumes on the
`bufferedamountlow` event. This prevents unbounded memory growth on large
files or fast senders.

### Progress

Sender tracks bytes sent / total. Receiver tracks bytes received against
`size` from `FileStart`. The UI shows a per-file progress bar for each.

## Error Handling

- Third peer attempts to join → `RoomFull`, rejected at the WS layer.
- Peer disconnects (WS close or data-channel close) → the other peer is
  notified, the UI shows "peer disconnected," and any in-flight transfer is
  aborted cleanly.
- ICE / connection failure → "couldn't establish direct connection" with a
  retry option.
- Unknown room id → "room not found / expired."
- Both peers leave → room removed from the registry.

## Testing

- **shared** — serde round-trip tests for every `SignalMsg` variant.
- **server** — async integration tests (Tokio): create room, join, room-full
  rejection, signal relay between two mock WS clients, disconnect cleanup.
- **client** — chunking / reassembly modeled as **pure functions**
  (bytes → frames → reassembled bytes), tested with `wasm-bindgen-test`. Live
  WebRTC is not unit-testable; it is verified manually with two browser tabs on
  localhost, documented as the e2e check.

## Tech Stack

- **server:** `axum`, `tokio`, `serde`, `serde_json`; rooms via
  `Mutex<HashMap>` or `dashmap`.
- **client:** `leptos` (CSR), `wasm-bindgen`, `web-sys` (RTC/File/Blob
  features), `gloo` as needed; built with **Trunk**.
- **shared:** `serde`, `serde_json`.
- STUN server configurable via environment variable, defaulting to a public
  STUN endpoint.

## Future Enhancements (out of scope for v1)

- TURN relay fallback (hybrid transport) for symmetric-NAT peers.
- Parallel multi-file transfers.
- Resumable transfers.
