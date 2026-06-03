# Testing Strategy — file-send

**Date:** 2026-06-03
**Status:** Approved design, ready for implementation plan

## Goal

Build a layered testing strategy that reproduces the real bugs we are hitting
(led by the sleep/reconnect connection-drop investigation — see the diagnostic
logging commits) and then expands into broad proactive coverage across every
layer. Tests that have no fix in the code yet are committed as **marked-failing**
(`#[ignore]` / Playwright `.fixme`) so they form an executable bug backlog that
flips green when the underlying gap is closed.

Priority order: **real bugs first, broad coverage second** (both are in scope).

## Background

`file-send` is anonymous P2P WebRTC file transfer: a Rust/Axum signaling server
brokers the handshake; file bytes flow browser-to-browser over a WebRTC data
channel and never touch the server. Workspace:

- `crates/shared` — JSON signaling wire types (`SignalMsg`).
- `crates/server` — Axum signaling server (`rooms`, `ws`, `limiter`); also
  serves the built client.
- `crates/client` — Leptos CSR (WASM) app; all WebRTC, chunking, UI.

Current test coverage:

- `crates/server/tests/signaling.rs` — 4 ws integration tests (create/join/relay,
  room-full, unknown-room, peer-left).
- Unit tests inside `rooms.rs`, `limiter.rs`, `shared/lib.rs`, `app.rs`
  (`normalize_code`, `resolve_origin`).
- **Client has no test coverage of the reconnect/handshake orchestration**, which
  is exactly where the sleep/reconnect bugs live.

Known failure catalogue (from the deferred-fixes notes + recent commits):
sleep/reconnect drops (active investigation), refresh/reclaim, mid-session
cancel, room TTL / idle expiry (no code yet), symmetric-NAT connect failure,
large-file backpressure, file-id semantics, per-IP rate-limit gaps behind a
proxy, third-peer/room-full.

## Approach

**Hybrid: thin harness, then bug-first.** Stand up a minimal, green smoke test
for each of the three test rigs first (server signaling already has this; add one
passing client-logic test and one green two-browser Playwright handshake). Once
the rigs are proven — especially the risky two-browser WebRTC automation —
attack the real bugs vertically (logic test + E2E per bug), leading with
sleep/reconnect, then backfill proactive coverage and the marked-failing backlog.

Rejected alternatives:
- *Bug-first vertical slices with no harness warm-up* — builds three rigs under
  pressure on the first bug; harness churn.
- *Layer-first horizontal* — systematic but slow to reach the real bugs;
  contradicts "lead with bugs."

## Architecture — three non-overlapping layers

```
crates/server/tests/          # Rust integration — signaling protocol & rooms
  signaling.rs                # expand: reclaim, rate-limit, ICE relay, TTL (marked)
crates/client/
  src/reconnect.rs            # NEW: extracted pure reducer (events -> actions)
  src/transfer_state.rs       # already pure-ish; harden + test
  tests/                      # NEW: native cargo tests for the reducers
    reconnect.rs
    transfer.rs
e2e/                          # NEW: Playwright (Node/TS) — the only non-Rust dir
  package.json
  playwright.config.ts
  fixtures/                   # two-peer harness, server boot, file gen
    server.ts
    peers.ts
    files.ts
  tests/
    handshake.spec.ts         # smoke (green first)
    transfer-integrity.spec.ts
    refresh-reclaim.spec.ts
    cancel.spec.ts
    sleep-reconnect.spec.ts   # the headline bug (.fixme at first)
    limits.spec.ts
docs/testing.md               # NEW: marked-failing backlog -> deferred-fix index
```

Each layer proves something the others cannot:

- **Rust server tests** → signaling/room *protocol* correctness (no browser).
- **Rust client reducer tests** → reconnect/role/teardown *decision logic* (no
  browser, no DOM, native compile — instant).
- **Playwright** → the *real WebRTC handshake + bytes on the wire + browser
  lifecycle* (sleep/offline/refresh) that only a real engine exercises.

`e2e/` builds the WASM client once (`trunk build`) and boots the real `server`
binary as a fixture, so E2E runs against actual production artifacts.

## Layer 1 — Reducer extraction (load-bearing refactor)

The reconnect/role logic currently lives in the big `match msg` inside
`Signaling::connect` in `app.rs`, plus the `on_open` Create-vs-Join decision and
the `RoomNotFound` reclaim branch. It interleaves *decisions* (what to do) with
*effects* (`build_pc`, `send`, `set_status`, `set_hash`, `spawn_local`). Extract
the decisions into a pure module `crates/client/src/reconnect.rs` — no `web_sys`,
no Leptos, no I/O. It compiles for native, so tests run with plain
`cargo test -p client`.

```rust
// Inputs: everything that drives a handshake decision.
enum Event {
    Open,
    Created { room: String },
    PeerJoined,
    Offer { sdp: String },
    Answer { sdp: String },
    Ice,
    PeerLeft,
    RoomFull,
    RoomNotFound,
}

// What the session knows when deciding.
struct Session {
    room_in_hash: Option<String>,   // from URL
    owns: Option<String>,           // from sessionStorage
    reclaim_tried: bool,
    has_pc: bool,
}

// Outputs: intents the App layer executes (the only side-effecting code).
enum Action {
    SendCreate,
    SendJoin { room: String },
    SendReclaim { room: String },
    BuildPcAndOffer,
    BuildPcAndAnswer { sdp: String },
    SetRemoteAnswer { sdp: String },
    AddIce,
    TeardownPc,
    SetStatus(Status),
    PersistRoom { room: String },
}

fn step(session: &mut Session, ev: Event) -> Vec<Action>;
```

`app.rs` shrinks to: translate inbound `SignalMsg` → `Event`, call `step`,
execute each returned `Action` against `web_sys`/Leptos. The thin glue stays
untested (E2E covers it end to end); the cracks live in `step`.

**Refactor safety:** this is surgery on working reconnect code. The current
behavior is documented (deferred-fixes notes); the first reducer tests encode
today's verified behavior as a characterization baseline **before** logic moves,
keeping the refactor behavior-preserving.

### Layer 1 tests (`crates/client/tests/reconnect.rs`)

- reload-as-owner: `Open` with hash+owns → `SendJoin`; then `RoomNotFound` →
  `SendReclaim` (once).
- **double-reclaim guard**: second `RoomNotFound` → `SetStatus(RoomNotFound)`,
  never a second `SendReclaim`.
- **reclaim-only-if-owner**: hash present but `owns = None` → `RoomNotFound`
  status, no reclaim (the room-hijack security guard).
- fresh start: `Open` with no hash → `SendCreate`.
- role dynamics: `PeerJoined` → `BuildPcAndOffer`; `Offer` → `BuildPcAndAnswer`.
- `PeerLeft` → `TeardownPc` + `SetStatus(PeerLeft)` (stale-connection cleanup).
- `Answer` / `Ice` with `has_pc = false` → no-op (today's "recv answer but no pc"
  log branch).

`transfer_state.rs` / `transfer.rs` logic (file-id assignment, cancel
transitions) gets matching native tests; the file-id-semantics gap (IDs restart
at 0 per batch; receiver never validates `FileEnd.id`) gets a marked-failing test
that asserts the desired validated behavior.

## Layer 2 — Server / signaling test expansion

Extend `crates/server/tests/signaling.rs` (real ws, ephemeral port) plus
registry-level unit tests where cheaper.

**Passing (regression guards):**

- Reclaim happy path: owner drops → room gone → `Reclaim{room}` re-creates it,
  reclaimer is initiator again.
- Reclaim hijack guard: `Reclaim{room}` while the room is still occupied →
  refused (the `contains()` security guard).
- Rate-limiter wired behavior: N failed `Join`s from one conn → subsequent joins
  get `RoomNotFound` even for a code that exists.
- ICE relay passthrough (today only the SDP offer is asserted).
- Keep: disconnect/`PeerLeft` propagation, room-full third peer.

**Marked-failing backlog (`#[ignore = "..."]`):**

- `#[ignore = "known: no room TTL — idle single-peer room never expires"]` — boot
  the server with a short test TTL, create a room, leave one peer, assert the room
  is gone after the window. Fails today (no timer). Flips green when TTL lands.
- `#[ignore = "known: rate limiter keys on socket IP, trusts no XFF — ineffective
  behind proxy"]` — documents the proxy gap as an executable note.

**Design constraint surfaced by the test:** room TTL must take its duration as
config (env var or constructor arg) so the test can set ~200ms instead of
sleeping 60s. The failing test defines this seam; the TTL fix must honor it.

## Layer 3 — Playwright E2E suite (`e2e/`)

The only non-Rust directory. Runs against real production artifacts: a
global-setup step runs `trunk build` once and boots the actual `server` binary on
an ephemeral port serving `dist`. Each test spins **two browser contexts**
(peer A, peer B) in one Chromium — two independent WebRTC peers; localhost host
candidates connect with no TURN needed.

**Harness fixtures (`e2e/fixtures/`):**

- `server.ts` — build + boot server, expose base URL, teardown.
- `peers.ts` — open two contexts; helpers to create-a-room (A), join-by-link (B),
  and wait for `Connected` status.
- `files.ts` — generate deterministic byte blobs (known hash) for
  `setInputFiles`; capture the receiver's download and hash-compare.

**Specs:**

| Spec | Scenarios | Status |
|---|---|---|
| `handshake.spec.ts` | A creates → B joins → both Connected (rig smoke) | green first |
| `transfer-integrity.spec.ts` | A→B byte-exact (hash match); bidirectional; multi-file batch; large file (exercises `bufferedamountlow` backpressure) | green |
| `refresh-reclaim.spec.ts` | owner refresh-while-waiting → reclaim; joiner mid-session refresh → reconnect; owner mid-session refresh → reconnect; missing room → error + "Start a new room" | green |
| `cancel.spec.ts` | cancel mid-transfer both directions → CANCELLED rows, no stuck "Active" | green |
| `sleep-reconnect.spec.ts` | connect → `context.setOffline(true)` on B (simulates sleep / NIC drop) → wait past timeout → `setOffline(false)` → assert recovery + a follow-up transfer succeeds | **`.fixme` at first** — the crack being chased |
| `limits.spec.ts` | third peer → RoomFull UI; bad code → RoomNotFound UI | green |

**Marked-failing / `.fixme` backlog:**

- `sleep-reconnect` — start here; expected to expose the real bug. Annotated with
  what "recovered" must mean so it is a precise target.
- `symmetric-NAT failure surfaces clear error` — **cannot be faithfully simulated
  on localhost loopback** (no NAT, no TURN). Marked `.fixme` with a note that it
  needs real network infra or a forced-ICE-failure injection. Flagged explicitly
  so it is never mistaken for covered.

**Flake control:** Playwright web-first assertions (`expect.poll`, auto-retrying
`toHaveText`) with generous WebRTC timeouts; `retries: 2` scoped to the E2E
project only. Rust tests stay zero-retry.

## CI wiring & backlog tracking

Existing CI (`.github/workflows/rust.yml`): `cargo build` + `cargo test` on
push/PR to main, then a docker push job on push.

**Three CI surfaces:**

1. **`cargo test` (existing job, required)** — automatically picks up the new
   `reconnect.rs` reducer tests and expanded server tests. Stays the blocking
   gate. `#[ignore]`'d crack-finders are skipped, so green stays green.
2. **`cargo test -- --ignored` (new job, non-blocking, `continue-on-error`)** —
   runs the marked-failing backlog and reports it. Surfaces the known cracks and
   gives early warning when one starts passing (the bug got fixed → remove the
   `#[ignore]`). A passing `--ignored` test is a signal, not a failure.
3. **`e2e` (new job)** — installs Node + Playwright browsers + `trunk` + the wasm
   target, builds, runs the green specs as a required gate. The heavy/flaky-prone
   specs (`sleep-reconnect`, symmetric-NAT) run on a **nightly schedule** or
   `workflow_dispatch` rather than every PR, keeping PR CI fast.

**Backlog tracking artifact:** `docs/testing.md` indexes every marked-failing test
→ the deferred fix it waits on (room TTL, file-id semantics, rate-limit-behind-
proxy, sleep/reconnect), cross-linked to the deferred-fixes notes. **Rule:** a
deferred fix is not "done" until its marker is removed and the test runs green in
the required gate. That converts "even if tests fail at first" into a closing loop
rather than a pile of skips.

## Build sequence

1. Reducer extraction (`reconnect.rs`) + characterization tests encoding current
   behavior; refactor `app.rs` to call `step` and execute `Action`s.
2. Thin green smoke per rig: one reducer test (have via #1), server signaling
   (exists), one Playwright `handshake.spec.ts`.
3. Bug-first vertical — sleep/reconnect: reducer tests for the decision logic +
   `sleep-reconnect.spec.ts` (`.fixme`), driving the fix.
4. Proactive backfill: remaining Layer-1 transfer tests, Layer-2 reclaim/rate-
   limit/ICE tests, remaining Playwright specs.
5. Marked-failing backlog: room TTL test (+ the TTL config seam), file-id test,
   rate-limit-proxy note, symmetric-NAT `.fixme`.
6. CI wiring: `--ignored` job, `e2e` job (PR-green + nightly-heavy), `docs/testing.md`.

## Out of scope

- TURN relay infrastructure (a documented future enhancement; symmetric-NAT E2E
  stays `.fixme` until it exists).
- Load/stress testing of the signaling server.
- Visual-regression testing of the brutalist frontend.
