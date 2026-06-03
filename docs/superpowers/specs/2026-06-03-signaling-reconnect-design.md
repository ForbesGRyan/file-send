# Signaling WebSocket Auto-Reconnect — Design

**Date:** 2026-06-03
**Status:** Approved design, ready for implementation plan

## Goal

Make a peer automatically recover its connection after the signaling WebSocket
drops (laptop sleep, network change, transient outage) **without a page reload**.
This fixes the headline "sleep/reconnect" bug: today a dropped peer is stuck
showing "Connected" while nothing works, and only a full refresh recovers it.

## Background — root cause (evidence-backed)

Investigated 2026-06-03 (systematic debugging) on branch `debug/sleep-reconnect`:

1. **The client opens the signaling WebSocket exactly once**, in the mount
   `Effect` in `app.rs` via `Signaling::connect`. `signaling.rs` has no `onclose`
   handler; a grep of `crates/client/src` finds no `onclose` / `reconnect` /
   `navigator.onLine` / `visibilitychange` anywhere. When the socket drops,
   nothing re-opens it. Recovery happens **only** via a page reload, which
   re-runs the mount Effect (fresh WS → `Join`). Sleep never reloads the page.

2. **Runtime proof.** Forcibly severing a peer's WS via Playwright
   `routeWebSocket` (`server.close()`): the client never re-opened the socket
   (route handler stayed at 1 invocation after 8 s), its status stayed
   `"Connected — ready to transfer"` despite a dead socket, and
   `iceConnectionState` went `disconnected` with nothing rebuilding the pc.

3. **The `setOffline` test could not reproduce this.** `context.setOffline(true)`
   does not sever an established loopback WebSocket, and WebRTC media/data flows
   bypass CDP network emulation entirely — both peers stayed `Connected` through
   a 6 s offline window and a transfer succeeded afterward. So the existing
   `e2e/tests/sleep-reconnect.spec.ts` (`.fixme`, `setOffline`) would *false-pass*.

The Plan-1 reducer (`reconnect.rs`) already encodes the correct re-handshake
*decisions* (`Event::Open` with a room in the hash → `SendJoin`; reclaim guards).
What is missing is purely the **transport trigger**: detect the drop, re-open the
WS, and re-fire `Event::Open`. This design adds exactly that.

## Scope

In scope: signaling WebSocket auto-reconnect (detection, backoff, re-handshake,
state reset, status, `online`-event wake trigger) and its tests.

Out of scope (deliberate, YAGNI): ICE-failure–triggered recovery (the rarer
"pc dies but WS survives" case); transfer resume (a partial transfer is marked
Cancelled, not resumed); `visibilitychange` triggers.

## Design decisions (resolved during brainstorming)

- **Trigger:** WS-reconnect only (sleep always drops the WS; the server's
  `PeerLeft` + the existing reducer handle the re-handshake).
- **Retry policy:** exponential backoff, retry **indefinitely**, capped.
  `next_backoff(attempt) = min(500 * 2^attempt, 15_000)` ms.
- **Wake trigger:** also reconnect immediately on the browser `online` event
  (resets backoff); the backoff timer is the fallback.
- **Status:** add a distinct `Status::Reconnecting` ("Reconnecting…").
- **In-flight transfers:** mark `Active` rows `Cancelled` on disconnect (reuse
  existing UI; no resume protocol).

## Architecture

**`Signaling` becomes a self-reconnecting transport** — the socket's lifecycle
lives with the thing that owns the socket. Caller-facing surface stays small.

```rust
pub struct Signaling { inner: Rc<SignalingInner> }

struct SignalingInner {
    url: String,
    ws: RefCell<WebSocket>,            // swapped on each reconnect
    on_msg: Rc<dyn Fn(SignalMsg)>,     // re-wired onto every new socket
    on_open: RefCell<Option<Rc<dyn Fn()>>>,
    on_disconnect: RefCell<Option<Rc<dyn Fn()>>>,
    attempt: Cell<u32>,                // backoff counter; reset to 0 on open
    closed: Cell<bool>,                // intentional close → do not reconnect
}
```

Responsibilities:

- **`Signaling`** — build the WS and wire `onmessage` / `onopen` / `onclose`.
  On `onopen`: `attempt = 0`, fire `on_open`. On `onclose` when `!closed`:
  fire `on_disconnect`, then schedule a reconnect at `next_backoff(attempt)`
  via `set_timeout`, incrementing `attempt`; the timer builds a fresh WS and
  re-wires it. Register a window `online` listener that — only if the current
  socket is not `OPEN` — cancels any pending timer, resets `attempt = 0`, and
  reconnects immediately. `send()` writes to the current socket. A `close()`
  method sets `closed = true` first so teardown does not trigger a reconnect.
- **App glue (`app.rs`)** — supplies two callbacks:
  - `on_open` → `step(&mut session, Event::Open)` + execute the actions
    (this already exists for the first connect; it now fires on every re-open).
  - `on_disconnect` → the ordered reset (below).
- **Reducer (`reconnect.rs`)** — **unchanged**.
- **`ui.rs`** — add `Status::Reconnecting` + label `"Reconnecting…"`.
- **Pure `next_backoff(attempt: u32) -> u32`** in `signaling.rs` — deterministic
  (no jitter), natively unit-testable.

### `on_disconnect` reset (ordered)

1. `set_status(Status::Reconnecting)`.
2. In-flight rows → Cancelled via a new pure helper
   `rows::mark_all_active_cancelled(list)` (flips every `Active` row, incoming or
   outgoing, to `Cancelled`; leaves `Done`/`Offered`/`Pending` untouched).
3. Tear down the stale pc — close it and `transfer = None` (shared with the
   reducer's `Action::TeardownPc` effect so the logic isn't duplicated).
4. Reset session: `has_pc = false`, `reclaim_tried = false`.

## Data flow

**Single-peer drop (B sleeps, A stays up):**

1. B `onclose` → `on_disconnect` reset (status `Reconnecting`, in-flight rows
   Cancelled, pc torn down, session reset).
2. `Signaling` schedules reconnect at `next_backoff(attempt)`; `attempt += 1`.
   Meanwhile the server saw B's socket close → sent `PeerLeft` to A → A's reducer
   `TeardownPc` + status `PeerLeft`; A remains in the room.
3. B's network returns → `online` listener → immediate reconnect (`attempt = 0`).
4. New WS opens → fire `on_open` → `step(Event::Open)`; room in hash → `SendJoin`.
5. Server: B re-joins A's room → notifies A `PeerJoined` → A `BuildPcAndOffer`;
   B receives the offer → `BuildPcAndAnswer`. Re-handshake completes → both
   `Connected`.

**Both peers slept (both WSs dropped):** both rooms were torn down server-side.
Both reconnect → `Event::Open` → `SendJoin` → `RoomNotFound`. The reducer's
existing reclaim logic handles it: the owner (`room_in_hash == owns`, and
`reclaim_tried` was reset in the disconnect handler) → `SendReclaim` re-creates
the room; the joiner re-`Join`s and they re-pair. No new logic — resetting
`reclaim_tried` on disconnect is what lets the once-only guard allow a fresh
reclaim per reconnect cycle.

**Intentional close** (tab close / teardown): `closed = true` before closing, so
`onclose` does not reconnect.

### Edge cases

- **Stale pc on reconnect:** always torn down + `has_pc` reset on disconnect, so
  the re-handshake builds a fresh pc; no stale-pc confusion.
- **Spurious `online` while healthy:** the immediate-reconnect is guarded on the
  current socket not being `OPEN`, so a stray `online` is a no-op.
- **Reconnect lands but the partner is gone:** handled by the normal
  `RoomNotFound`/reclaim path.

## Testing

**Pure unit tests (`cargo test -p client`, native):**
- `next_backoff`: ramp `500, 1000, 2000, 4000, 8000, 15000, 15000…` (cap holds).
- `rows::mark_all_active_cancelled`: `Active` rows (both directions) → `Cancelled`;
  `Done`/`Offered`/`Pending` unchanged.

**E2E — replace the false-green test.** Delete `e2e/tests/sleep-reconnect.spec.ts`
(the `setOffline` `.fixme`). Add `e2e/tests/reconnect.spec.ts` using the
`routeWebSocket` severance validated during debugging:
- Connect a pair with B's `/ws` proxied. Drop B's WS (`server.close()`).
- Assert B's status → `"Reconnecting…"`, then the route handler is invoked again
  (client re-opened the WS), then both → `Connected`.
- Assert a transfer works after recovery.
- (Cheap second case) an in-flight transfer's row shows `Cancelled` after the drop.

Committed as a **real, initially-failing** test (fails today → green after the
fix), not a `.fixme`.

**Docs.** `docs/testing.md`: drop the sleep-reconnect `.fixme` backlog row (it
becomes a passing gate test); keep the symmetric-NAT row. Add a one-line note
that `setOffline` is unsuitable for WebRTC/loopback severance so nobody reaches
for it again.

## Files touched

- `crates/client/src/signaling.rs` — self-reconnecting transport + `next_backoff`.
- `crates/client/src/app.rs` — provide `on_open` (already) + `on_disconnect`;
  share the pc-teardown helper.
- `crates/client/src/ui.rs` — `Status::Reconnecting` + label.
- `crates/client/src/rows.rs` — `mark_all_active_cancelled` pure helper + tests.
- `e2e/tests/reconnect.spec.ts` (new), delete `e2e/tests/sleep-reconnect.spec.ts`.
- `docs/testing.md` — backlog table update + `setOffline` note.

## Out of scope / future

- ICE-failure–triggered recovery (pc dies, WS survives).
- Transfer resume after reconnect.
- `visibilitychange` reconnect trigger.
