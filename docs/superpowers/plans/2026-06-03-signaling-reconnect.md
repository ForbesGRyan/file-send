# Signaling WebSocket Auto-Reconnect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a peer automatically recover after its signaling WebSocket drops (sleep/network loss) without a page reload.

**Architecture:** Turn `Signaling` into a self-reconnecting transport: on the WS `onclose` it fires an app-supplied `on_disconnect` reset, then reconnects with capped exponential backoff (and immediately on the browser `online` event); on each (re)open it fires `on_open`, which the app already wires to `step(Event::Open)` → re-`Join`. The pure reducer (`reconnect.rs`) is unchanged. A new `Status::Reconnecting` stops the UI lying, and in-flight rows are marked `Cancelled` on drop.

**Tech Stack:** Rust (edition 2024), Leptos CSR/WASM client, web-sys WebSocket; Playwright (Node/TS) E2E.

**Spec:** `docs/superpowers/specs/2026-06-03-signaling-reconnect-design.md`.

---

## File structure

- Modify: `crates/client/src/signaling.rs` — self-reconnecting transport + pure `next_backoff`.
- Modify: `crates/client/src/ui.rs` — `Status::Reconnecting` + label.
- Modify: `crates/client/src/rows.rs` — `mark_all_active_cancelled` pure helper + tests.
- Modify: `crates/client/src/app.rs` — `teardown_pc` shared helper, `on_disconnect` wiring.
- Create: `e2e/tests/reconnect.spec.ts`; Delete: `e2e/tests/sleep-reconnect.spec.ts`.
- Modify: `docs/testing.md` — backlog table update + `setOffline` note.

---

## Task 1: `next_backoff` pure function

**Files:**
- Modify: `crates/client/src/signaling.rs`

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/client/src/signaling.rs` (the file currently has no `tests` module):

```rust
#[cfg(test)]
mod tests {
    use super::next_backoff;

    #[test]
    fn backoff_ramps_then_caps() {
        // 500 * 2^attempt, capped at 15_000 ms.
        assert_eq!(next_backoff(0), 500);
        assert_eq!(next_backoff(1), 1_000);
        assert_eq!(next_backoff(2), 2_000);
        assert_eq!(next_backoff(3), 4_000);
        assert_eq!(next_backoff(4), 8_000);
        // 500 * 2^5 = 16_000 -> capped.
        assert_eq!(next_backoff(5), 15_000);
        assert_eq!(next_backoff(6), 15_000);
        // Large attempts never overflow or drop below the cap.
        assert_eq!(next_backoff(40), 15_000);
        assert_eq!(next_backoff(100), 15_000);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p client backoff_ramps_then_caps`
Expected: FAIL to compile — `next_backoff` doesn't exist.

- [ ] **Step 3: Implement `next_backoff`**

Add near the top of `crates/client/src/signaling.rs`, after the `use` lines:

```rust
/// Base reconnect delay in ms; doubles each attempt up to `BACKOFF_CAP_MS`.
const BACKOFF_BASE_MS: u32 = 500;
/// Maximum reconnect delay in ms.
const BACKOFF_CAP_MS: u32 = 15_000;

/// Reconnect delay for the Nth consecutive attempt (0-based): exponential
/// backoff `500 * 2^attempt`, capped at 15s. Pure, for unit testing and so the
/// reconnect timer and any future caller share one definition. Uses `u64`
/// internally so a large `attempt` saturates to the cap instead of overflowing.
pub fn next_backoff(attempt: u32) -> u32 {
    let shifted = (BACKOFF_BASE_MS as u64).checked_shl(attempt).unwrap_or(u64::MAX);
    shifted.min(BACKOFF_CAP_MS as u64) as u32
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p client backoff_ramps_then_caps`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/signaling.rs
git commit -m "client: add next_backoff pure reconnect-delay function"
```

---

## Task 2: `Status::Reconnecting`

**Files:**
- Modify: `crates/client/src/ui.rs`

- [ ] **Step 1: Write the failing test**

In `crates/client/src/ui.rs`, the `tests` module has `status_labels_cover_every_variant`. Add a new assertion line inside it (after the `RoomNotFound` line):

```rust
        assert_eq!(Status::Reconnecting.label(), "Reconnecting…");
```

(The `…` is U+2026, matching the existing `WaitingForPeer` label style.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p client status_labels_cover_every_variant`
Expected: FAIL to compile — `Status::Reconnecting` doesn't exist.

- [ ] **Step 3: Implement the variant + label**

In `crates/client/src/ui.rs`, add `Reconnecting` to the `Status` enum (after `Connected`, since it's a post-connection state):

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    WaitingForPeer,
    Connecting,
    Connected,
    Reconnecting,
    PeerLeft,
    RoomFull,
    RoomNotFound,
    Error(String),
}
```

And add its arm to `label()` (after the `Connected` arm):

```rust
            Status::Reconnecting => "Reconnecting…".into(),
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p client status_labels_cover_every_variant`
Expected: PASS.

- [ ] **Step 5: Verify the reducer still matches exhaustively**

The reducer (`reconnect.rs`) never constructs `Status::Reconnecting`, and `Status` is only matched in `ui.rs::label`, so adding a variant is safe. Confirm the crate compiles:

Run: `cargo test -p client`
Expected: PASS (no non-exhaustive-match errors).

- [ ] **Step 6: Commit**

```bash
git add crates/client/src/ui.rs
git commit -m "client: add Status::Reconnecting"
```

---

## Task 3: `rows::mark_all_active_cancelled`

**Files:**
- Modify: `crates/client/src/rows.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/client/src/rows.rs`:

```rust
    #[test]
    fn mark_all_active_cancelled_hits_only_active_rows() {
        let mut list = Vec::new();
        // An active incoming download and an active outgoing send.
        recv_progress(&mut list, 1, "in", 30.0, 100.0, 0.0); // Active incoming
        send_progress(&mut list, 2, "out", 40.0, 100.0); // Active outgoing
        // Rows that must be left alone:
        send_progress(&mut list, 3, "done", 100.0, 100.0); // Done
        incoming_offer(&mut list, &meta(4, "offered", 1.0)); // Offered
        push_outgoing_pending(&mut list, 5, "pending", 1.0); // Pending

        mark_all_active_cancelled(&mut list);

        let by_id = |id: u64| list.iter().find(|r| r.id == id).unwrap().state.clone();
        assert_eq!(by_id(1), TransferState::Cancelled);
        assert_eq!(by_id(2), TransferState::Cancelled);
        assert_eq!(by_id(3), TransferState::Done); // untouched
        assert_eq!(by_id(4), TransferState::Offered); // untouched
        assert_eq!(by_id(5), TransferState::Pending); // untouched
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p client mark_all_active_cancelled_hits_only_active_rows`
Expected: FAIL to compile — function doesn't exist.

- [ ] **Step 3: Implement the helper**

Add to `crates/client/src/rows.rs` (after `mark_cancelled_remote`, near the other cancel helpers):

```rust
/// Mark every in-flight (`Active`) row — incoming or outgoing — as `Cancelled`.
/// Used when the signaling connection drops mid-transfer: the data channel dies
/// with it, so any active transfer is dead and cannot resume. Leaves
/// `Done`/`Offered`/`Pending`/`Declined`/`Cancelled` rows untouched.
pub fn mark_all_active_cancelled(list: &mut [Row]) {
    for r in list.iter_mut() {
        if r.state == TransferState::Active {
            r.state = TransferState::Cancelled;
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p client mark_all_active_cancelled_hits_only_active_rows`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/rows.rs
git commit -m "client: add rows::mark_all_active_cancelled"
```

---

## Task 4: `Signaling` self-reconnecting transport

This rewrites `signaling.rs`. It is web-sys glue (no native unit test beyond `next_backoff`); correctness is verified by the wasm build here and the E2E test in Task 6. Keep `next_backoff` and its test (Task 1).

**Files:**
- Modify: `crates/client/src/signaling.rs`

- [ ] **Step 1: Replace the `Signaling` implementation**

Replace everything in `crates/client/src/signaling.rs` **except** the `next_backoff` function/constants and the `#[cfg(test)] mod tests` block with the following. (Keep the module doc comment updated, the `next_backoff` block from Task 1, and the tests.)

Final file shape — module doc, imports, `next_backoff` + constants (from Task 1), then:

```rust
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shared::SignalMsg;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

/// Live JS closures for one socket. Held so they outlive the socket and are
/// dropped (freeing the old socket's handlers) when a reconnect installs a new
/// socket's closures.
struct SocketCallbacks {
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onopen: Closure<dyn FnMut()>,
    _onclose: Closure<dyn FnMut()>,
}

/// Shared, reference-counted signaling state. The `online` listener and the
/// per-socket closures hold clones of this `Rc`, forming a deliberate cycle:
/// `Signaling` is a page-lifetime singleton, so it is never meant to be dropped
/// (mirrors the original `cb.forget()` pattern).
struct SignalingInner {
    url: String,
    ws: RefCell<WebSocket>,
    on_msg: Rc<dyn Fn(SignalMsg)>,
    on_open: RefCell<Option<Rc<dyn Fn()>>>,
    on_disconnect: RefCell<Option<Rc<dyn Fn()>>>,
    /// Consecutive failed-reconnect counter; reset to 0 on a successful open.
    attempt: Cell<u32>,
    /// Set before an intentional close so `onclose` does not reconnect.
    closed: Cell<bool>,
    callbacks: RefCell<Option<SocketCallbacks>>,
    _online: RefCell<Option<Closure<dyn FnMut()>>>,
}

/// Owns a browser WebSocket and transparently reconnects it (capped exponential
/// backoff, plus an immediate retry on the browser `online` event). Routes
/// inbound `SignalMsg`s to `on_msg`; fires `on_open` on every (re)open and
/// `on_disconnect` on every unexpected drop.
#[derive(Clone)]
pub struct Signaling {
    inner: Rc<SignalingInner>,
}

impl Signaling {
    /// Connect to the signaling endpoint (relative `/ws` on the current host).
    /// `on_msg` is called for every successfully parsed inbound message, across
    /// reconnects.
    pub fn connect(on_msg: impl Fn(SignalMsg) + 'static) -> Result<Self, JsValue> {
        let location = web_sys::window().unwrap().location();
        let proto = if location.protocol()? == "https:" { "wss" } else { "ws" };
        let host = location.host()?;
        let url = format!("{proto}://{host}/ws");

        let ws = WebSocket::new(&url)?;
        let inner = Rc::new(SignalingInner {
            url,
            ws: RefCell::new(ws),
            on_msg: Rc::new(on_msg),
            on_open: RefCell::new(None),
            on_disconnect: RefCell::new(None),
            attempt: Cell::new(0),
            closed: Cell::new(false),
            callbacks: RefCell::new(None),
            _online: RefCell::new(None),
        });
        wire_socket(&inner);
        register_online(&inner);
        Ok(Self { inner })
    }

    /// Send a signaling message on the current socket (no-op if not open yet).
    pub fn send(&self, msg: &SignalMsg) {
        if let Ok(json) = serde_json::to_string(msg) {
            let _ = self.inner.ws.borrow().send_with_str(&json);
        }
    }

    /// Register a callback fired on every (re)connect once the socket opens.
    pub fn on_open(&self, f: impl Fn() + 'static) {
        *self.inner.on_open.borrow_mut() = Some(Rc::new(f));
    }

    /// Register a callback fired whenever the socket drops unexpectedly (before
    /// the reconnect is scheduled).
    pub fn on_disconnect(&self, f: impl Fn() + 'static) {
        *self.inner.on_disconnect.borrow_mut() = Some(Rc::new(f));
    }
}

/// Wire `onmessage`/`onopen`/`onclose` onto the inner's current socket and store
/// the closures (dropping any previous socket's closures).
fn wire_socket(inner: &Rc<SignalingInner>) {
    let ws = inner.ws.borrow().clone();

    let onmessage = {
        let on_msg = inner.on_msg.clone();
        Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            if let Some(text) = e.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<SignalMsg>(&text) {
                    on_msg(msg);
                }
            }
        })
    };
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

    let onopen = {
        let inner = inner.clone();
        Closure::<dyn FnMut()>::new(move || {
            inner.attempt.set(0);
            if let Some(f) = inner.on_open.borrow().as_ref() {
                f();
            }
        })
    };
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));

    let onclose = {
        let inner = inner.clone();
        Closure::<dyn FnMut()>::new(move || {
            if inner.closed.get() {
                return;
            }
            crate::log::clog("[ws] closed -> reconnecting");
            if let Some(f) = inner.on_disconnect.borrow().as_ref() {
                f();
            }
            schedule_reconnect(&inner);
        })
    };
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

    *inner.callbacks.borrow_mut() = Some(SocketCallbacks {
        _onmessage: onmessage,
        _onopen: onopen,
        _onclose: onclose,
    });
}

/// Schedule a reconnect after `next_backoff(attempt)` ms, incrementing the
/// attempt counter. Runs out of the `onclose` call stack via `set_timeout`.
fn schedule_reconnect(inner: &Rc<SignalingInner>) {
    let delay = next_backoff(inner.attempt.get()) as i32;
    inner.attempt.set(inner.attempt.get() + 1);
    let inner2 = inner.clone();
    let cb = Closure::once_into_js(move || reconnect_now(&inner2));
    let _ = web_sys::window()
        .unwrap()
        .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), delay);
}

/// Build a fresh socket and wire it, unless intentionally closed or a connect is
/// already in flight/open (so a backoff tick and an `online` event can't double-
/// connect). On construction failure, retry on the next backoff tick.
fn reconnect_now(inner: &Rc<SignalingInner>) {
    if inner.closed.get() {
        return;
    }
    let state = inner.ws.borrow().ready_state();
    if state == WebSocket::OPEN || state == WebSocket::CONNECTING {
        return;
    }
    match WebSocket::new(&inner.url) {
        Ok(ws) => {
            *inner.ws.borrow_mut() = ws;
            wire_socket(inner);
        }
        Err(_) => schedule_reconnect(inner),
    }
}

/// Reconnect immediately when the browser regains connectivity (e.g. wake from
/// sleep), resetting the backoff — but only if the current socket isn't already
/// open/connecting.
fn register_online(inner: &Rc<SignalingInner>) {
    let inner2 = inner.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        if inner2.closed.get() {
            return;
        }
        let state = inner2.ws.borrow().ready_state();
        if state == WebSocket::OPEN || state == WebSocket::CONNECTING {
            return;
        }
        crate::log::clog("[ws] online -> reconnecting now");
        inner2.attempt.set(0);
        reconnect_now(&inner2);
    });
    let _ = web_sys::window()
        .unwrap()
        .add_event_listener_with_callback("online", cb.as_ref().unchecked_ref());
    *inner._online.borrow_mut() = Some(cb);
}
```

- [ ] **Step 2: Build for wasm to confirm it compiles**

Run: `cargo build -p client --target wasm32-unknown-unknown`
Expected: compiles cleanly.

If it fails with `no method named set_onclose` or `add_event_listener_with_callback` or missing `WebSocket::OPEN`, the needed web-sys features are missing — add them to `crates/client/Cargo.toml` under the `web-sys` dependency `features = [...]` list (candidates: `"WebSocket"` already present; add `"Window"`, `"EventTarget"` if not present). Re-run until clean. (`set_timeout_with_callback_and_timeout_and_arguments_0` is already used in `transfer.rs`, so `Window` is almost certainly already enabled.)

- [ ] **Step 3: Run the native test suite (pure pieces unaffected)**

Run: `cargo test -p client`
Expected: PASS (all existing tests + `next_backoff` + `mark_all_active_cancelled` + the Status test). `signaling.rs`'s web-sys code isn't unit-tested natively; that's expected.

- [ ] **Step 4: Commit**

```bash
git add crates/client/src/signaling.rs
git commit -m "client: make Signaling a self-reconnecting transport"
```

---

## Task 5: Wire `on_disconnect` + shared `teardown_pc` in `app.rs`

**Files:**
- Modify: `crates/client/src/app.rs`

- [ ] **Step 1: Add a shared `teardown_pc` helper inside the mount Effect**

In `crates/client/src/app.rs`, inside the mount `Effect::new(move |_| { ... })`, **after** the `build_pc` closure definition and **before** the `let last_ice` line (currently ~line 283), add:

```rust
            // Close the current peer connection and drop the transfer. Shared by
            // the reducer's `TeardownPc` action and the signaling `on_disconnect`
            // reset so the teardown logic lives in one place.
            let teardown_pc: Rc<dyn Fn()> = {
                let pc = pc.clone();
                let transfer = transfer.clone();
                Rc::new(move || {
                    if let Some(peer) = pc.borrow_mut().take() {
                        peer.close();
                    }
                    *transfer.borrow_mut() = None;
                })
            };
```

- [ ] **Step 2: Use `teardown_pc` in the `Action::TeardownPc` arm**

In the `execute` closure, first capture it: add `let teardown_pc = teardown_pc.clone();` to the capture block at the top of the `execute` definition (alongside `let build_pc = build_pc.clone();` etc.).

Then replace the `Action::TeardownPc` arm body:

```rust
                    Action::TeardownPc => {
                        crate::log::clog("[rtc] PeerLeft -> closing pc");
                        if let Some(peer) = pc.borrow_mut().take() {
                            peer.close();
                        }
                        *transfer.borrow_mut() = None;
                    }
```

with:

```rust
                    Action::TeardownPc => {
                        crate::log::clog("[rtc] PeerLeft -> closing pc");
                        teardown_pc();
                    }
```

(The `pc` and `transfer` captures in `execute` are still used by other arms — `SetRemoteAnswer`, `AddIce` use `pc`; leave those captures in place.)

- [ ] **Step 3: Wire `on_disconnect` after building `signaling`**

After the `let signaling = match Signaling::connect({ ... }) { ... };` block (currently ending ~line 448) and **before** the `signaling.on_open(...)` block, add:

```rust
            // On an unexpected socket drop: surface "Reconnecting", cancel any
            // in-flight transfers (the data channel died with the socket), tear
            // down the stale pc, and reset the session so the reconnect handshake
            // starts clean. `Signaling` then reconnects (backoff / `online`) and
            // fires `on_open`, which re-Joins the room via the reducer.
            {
                let teardown_pc = teardown_pc.clone();
                let session = session.clone();
                signaling.on_disconnect(move || {
                    set_status.set(Status::Reconnecting);
                    set_items.update(|list| rows::mark_all_active_cancelled(list));
                    teardown_pc();
                    let mut s = session.borrow_mut();
                    s.has_pc = false;
                    s.reclaim_tried = false;
                });
            }
```

- [ ] **Step 4: Run the native test suite**

Run: `cargo test -p client`
Expected: PASS (no regressions).

- [ ] **Step 5: Build for wasm**

Run: `cargo build -p client --target wasm32-unknown-unknown`
Expected: compiles cleanly. (If a borrow/move error appears, ensure `teardown_pc` is cloned into both `execute` and the `on_disconnect` block, and that `session`/`set_status`/`set_items` are cloned/copied as the surrounding code does — `set_status`/`set_items` are `Copy` Leptos setters.)

- [ ] **Step 6: Commit**

```bash
git add crates/client/src/app.rs
git commit -m "client: reset + show Reconnecting on signaling drop, recover on reconnect"
```

---

## Task 6: E2E reconnect test + docs

**Files:**
- Create: `e2e/tests/reconnect.spec.ts`
- Delete: `e2e/tests/sleep-reconnect.spec.ts`
- Modify: `docs/testing.md`

- [ ] **Step 1: Delete the false-green setOffline test**

```bash
git rm e2e/tests/sleep-reconnect.spec.ts
```

- [ ] **Step 2: Create `e2e/tests/reconnect.spec.ts`**

This drops B's signaling WebSocket via a `routeWebSocket` proxy and asserts B surfaces `Reconnecting…`, re-opens the socket, recovers to `Connected`, and can transfer again. The proxy forwards messages both ways; to drop the connection we close **both** the server side (frees the room slot so B can re-Join) and the client side (so B's page sees `onclose`).

```ts
import { test, expect } from "@playwright/test";
import { bytes, sendFile, expectDownloadMatches } from "../fixtures/files";

const CONNECTED = "Connected — ready to transfer";
const RECONNECTING = "Reconnecting…";

test("peer reconnects after its signaling WebSocket drops", async ({ browser }) => {
  test.setTimeout(60_000);
  const ctxA = await browser.newContext({ acceptDownloads: true });
  const ctxB = await browser.newContext({ acceptDownloads: true });
  const a = await ctxA.newPage();
  const b = await ctxB.newPage();

  // Proxy B's signaling WS so we can drop it mid-session and count reconnects.
  // Each /ws connection B opens runs this handler, so bRoutes counts (re)opens.
  let bRoutes = 0;
  let lastWs: import("@playwright/test").WebSocketRoute | null = null;
  let lastServer: import("@playwright/test").WebSocketRoute | null = null;
  await b.routeWebSocket(/\/ws$/, (ws) => {
    bRoutes++;
    const server = ws.connectToServer();
    lastWs = ws;
    lastServer = server;
    ws.onMessage((m) => server.send(m));
    server.onMessage((m) => ws.send(m));
  });

  await a.goto("/");
  const code = await a.locator(".code").innerText();
  await b.goto(`/#/room/${code}`);
  await expect(a.locator(".status")).toHaveText(CONNECTED);
  await expect(b.locator(".status")).toHaveText(CONNECTED);
  expect(bRoutes).toBe(1);

  // Drop B's signaling WS: free the server slot AND close the client side so
  // B's page fires `onclose`.
  lastServer!.close();
  lastWs!.close();

  // B surfaces Reconnecting, re-opens the socket (bRoutes >= 2), then both
  // sides recover to Connected.
  await expect(b.locator(".status")).toHaveText(RECONNECTING, { timeout: 10_000 });
  await expect.poll(() => bRoutes, { timeout: 20_000 }).toBeGreaterThanOrEqual(2);
  await expect(b.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });
  await expect(a.locator(".status")).toHaveText(CONNECTED, { timeout: 20_000 });

  // A transfer works after recovery.
  const content = bytes(64 * 1024, 9);
  await sendFile(a, "after-reconnect.bin", content);
  await expectDownloadMatches(b, content, () => b.locator("button.accept").click());

  await ctxA.close();
  await ctxB.close();
});
```

- [ ] **Step 3: Run the new test**

Run (from `e2e/`, long timeout — a cold first run rebuilds the server): `npx playwright test reconnect`
Expected: 1 passed.

Investigate REAL causes if it fails (do not weaken assertions):
- B never shows `Reconnecting…` → B's `onclose` didn't fire. Confirm both `lastServer.close()` and `lastWs.close()` ran; check B's console for `[ws] closed -> reconnecting`. If closing the route doesn't propagate a close to the page socket in this Playwright version, that's the lever to fix in the test.
- `bRoutes` stays 1 → the client isn't re-opening the socket (the fix's reconnect path) — capture B's console (`[ws] online`/backoff) and re-check Task 4.
- B re-opens but hits `Room is full` → the server slot wasn't freed; ensure `lastServer.close()` ran before/with the reconnect (the slot must free so B can re-Join). The reducer's reclaim path also covers a fully-gone room.

- [ ] **Step 4: Run the full E2E suite**

Run (from `e2e/`): `npx playwright test`
Expected: all pass + 1 skipped (the symmetric-NAT `.fixme` in `limits.spec.ts`). The old sleep-reconnect skip is gone; the new reconnect test is a passing gate test.

- [ ] **Step 5: Update `docs/testing.md`**

In `docs/testing.md`, in the marked-failing backlog table, **remove** the `sleep-reconnect.spec.ts` row (it's now a passing gate test, not a fixme). Keep the `limits.spec.ts` symmetric-NAT row and the two `#[ignore]` Rust rows. Then add this note under the table:

```markdown
> **Note on simulating disconnects:** Playwright's `context.setOffline(true)`
> does **not** sever an already-established loopback WebSocket, and WebRTC
> media/data flows bypass CDP network emulation entirely — so it cannot
> reproduce a sleep/network drop. `reconnect.spec.ts` instead severs the
> signaling WebSocket via a `routeWebSocket` proxy, which works deterministically.
```

- [ ] **Step 6: Commit**

```bash
git add e2e/tests/reconnect.spec.ts docs/testing.md
git commit -m "e2e: replace setOffline fixme with routeWebSocket reconnect test"
```

---

## Self-review

**Spec coverage:**
- WS-reconnect only, leveraging server `PeerLeft` + existing reducer → Tasks 4, 5 (reducer untouched). ✓
- Exponential backoff, indefinite, capped (`next_backoff`) → Task 1, used in Task 4. ✓
- `online`-event immediate reconnect (reset backoff, guard if already open) → Task 4 (`register_online`). ✓
- `Status::Reconnecting` + label → Task 2; set on drop → Task 5. ✓
- In-flight rows → `Cancelled` on drop (`mark_all_active_cancelled`) → Task 3, used in Task 5. ✓
- `on_disconnect` ordered reset (status, cancel rows, teardown pc, reset `has_pc`+`reclaim_tried`) → Task 5. ✓
- Shared pc teardown (no duplication) → Task 5 (`teardown_pc`). ✓
- Intentional-close guard (`closed`) so teardown doesn't reconnect → Task 4. ✓
- Spurious-`online`/double-connect guards (ready_state checks) → Task 4. ✓
- Both-peers-slept → reclaim: handled by the existing reducer once `reclaim_tried` is reset on disconnect (Task 5); no new code. ✓
- Pure unit tests (`next_backoff`, `mark_all_active_cancelled`) → Tasks 1, 3. ✓
- E2E routeWebSocket-severance test replacing the setOffline `.fixme` → Task 6. ✓
- `docs/testing.md` update + `setOffline` note → Task 6. ✓

**Placeholder scan:** No TBD/TODO-as-work. Task 4's "add web-sys features if the build complains" is concrete contingency guidance with named candidates, not a placeholder. All code steps show complete code.

**Type consistency:** `next_backoff(u32) -> u32`, `Status::Reconnecting`, `rows::mark_all_active_cancelled(&mut [Row])`, `Signaling::{connect, send, on_open, on_disconnect}`, and `teardown_pc: Rc<dyn Fn()>` are used consistently across tasks. `Signaling` keeps the same public method names the existing `app.rs` already calls (`connect`, `send`, `on_open`), plus the new `on_disconnect`.

**Note for the implementer:** Task 6's `routeWebSocket` close semantics are the one empirically-uncertain spot (does closing the route propagate `onclose` to the page socket in this Playwright version?). The task closes both sides and gives a diagnosis path; if recovery still doesn't trigger, that's a test-harness lever to adjust, not an app-fix — verify B's `[ws] closed` console line to tell them apart.
```
