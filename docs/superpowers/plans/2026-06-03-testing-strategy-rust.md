# Testing Strategy — Plan 1: Rust (reducer + marked-failing backlog) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the client's reconnect/role-selection logic into a pure, natively-testable reducer (attacking the sleep/reconnect bug), and add marked-failing tests that pin the two known unimplemented gaps (room TTL, file-id validation), wired into CI as a non-blocking backlog.

**Architecture:** The reconnect decisions currently live tangled with side effects inside the `match msg` in `app.rs`. We pull the *decisions* into `crates/client/src/reconnect.rs` as `step(&mut Session, Event) -> Vec<Action>` — no `web_sys`, no Leptos runtime — so the sleep/reconnect logic becomes deterministic unit tests run by plain `cargo test`. `app.rs` shrinks to: map inbound `SignalMsg` → `Event`, call `step`, execute each `Action`. Two `#[ignore]`'d tests define seams for fixes that don't exist yet (room TTL idle-expiry; receiver validating `FileEnd.id`).

**Tech Stack:** Rust (edition 2024), Leptos CSR client, Axum server. Tests are plain `#[test]` functions compiled for the host target (the existing `cargo test` CI job already builds the client crate natively).

**Scope note:** Playwright two-peer E2E (the third layer in the spec) is a separate subsystem and gets its own plan: `docs/superpowers/plans/2026-06-03-testing-strategy-e2e.md` (written after this one). This plan is independently shippable: it improves client testability and lands the Rust half of the backlog.

**Reality check vs spec:** `crates/server/src/ws.rs` already unit-tests reclaim happy-path (`reclaim_recreates_room_under_same_id_and_accepts_a_joiner`), the hijack guard (`reclaim_cannot_displace_a_live_room`), rate limiting (`repeated_failed_joins_from_one_ip_get_rate_limited`), and ICE relay (`join_notifies_initiator_then_relays_sdp_and_ice`). The spec's "Layer 2 passing tests" are therefore already done — this plan does NOT re-add them. The only new server work is the room-TTL seam (Task 7).

---

## File structure

- Create: `crates/client/src/reconnect.rs` — pure reducer: `Event`, `Session`, `Action`, `step`.
- Modify: `crates/client/src/main.rs` — register `mod reconnect;`.
- Modify: `crates/client/src/ui.rs:7` — add `Debug` to `Status`'s derive (needed for `assert_eq!` on `Action`).
- Modify: `crates/client/src/app.rs:280-433` — replace the inline `match msg` body with translate→`step`→execute.
- Modify: `crates/client/src/transfer_state.rs` — add `finalize_decision_checked` seam + `#[ignore]` file-id test.
- Modify: `crates/server/src/rooms.rs` — add `touch` + `reap_idle` seam + `#[ignore]` TTL test.
- Modify: `.github/workflows/rust.yml` — add non-blocking `ignored-backlog` job.

---

## Task 1: Reducer module + the open-handshake decision

**Files:**
- Create: `crates/client/src/reconnect.rs`
- Modify: `crates/client/src/main.rs`
- Modify: `crates/client/src/ui.rs:7`

- [ ] **Step 1: Add `Debug` to `Status` so `Action` can derive it**

In `crates/client/src/ui.rs`, change line 7 from:

```rust
#[derive(Clone, PartialEq)]
pub enum Status {
```

to:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
```

- [ ] **Step 2: Register the module**

In `crates/client/src/main.rs`, add `mod reconnect;` alongside the other `mod` declarations (match the existing ordering/style).

- [ ] **Step 3: Write the reducer types and a `step` that only handles `Open`, with the first failing tests**

Create `crates/client/src/reconnect.rs`:

```rust
//! Pure reducer for the signaling/reconnect handshake.
//!
//! The browser-facing half ([`crate::app`]) owns the WebSocket, the peer
//! connection, session storage, and the URL hash. This module owns only the
//! *decisions*: given what the session knows and one inbound signaling event,
//! what should happen? It has no `web_sys` or Leptos dependency, so the
//! reconnect logic — where the sleep/reconnect bugs live — is unit-testable on
//! the host target with plain `cargo test`.

use crate::ui::Status;

/// An inbound signaling event, distilled from a `shared::SignalMsg` (the SDP
/// `kind` split into `Offer`/`Answer`, payloads kept only where a later action
/// needs them).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The signaling socket opened.
    Open,
    /// The server created (or reclaimed) our room under `room`.
    Created { room: String },
    /// The other peer joined; we are the side that offers.
    PeerJoined,
    /// We received an SDP offer; we are the side that answers.
    Offer { sdp: String },
    /// We received an SDP answer to the offer we sent.
    Answer { sdp: String },
    /// We received an ICE candidate.
    Ice,
    /// The partner left/disconnected.
    PeerLeft,
    /// The room is full.
    RoomFull,
    /// The room we tried to (re)join does not exist.
    RoomNotFound,
}

/// What the session knows when deciding. Seeded by `app.rs` at startup from the
/// URL hash and `sessionStorage`, then maintained by `step`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    /// Room id present in the URL hash (`#/room/<id>`), if any.
    pub room_in_hash: Option<String>,
    /// Room id this tab created and therefore may reclaim (from `sessionStorage`).
    pub owns: Option<String>,
    /// Whether a reclaim has already been attempted this session (reclaim at most once).
    pub reclaim_tried: bool,
    /// Whether a live peer connection currently exists.
    pub has_pc: bool,
}

/// A side-effecting intent for `app.rs` to execute. This is the only place
/// side effects originate; `step` itself performs none.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send `Create` to the signaling server.
    SendCreate,
    /// Send `Join { room }`.
    SendJoin { room: String },
    /// Send `Reclaim { room }`.
    SendReclaim { room: String },
    /// Build a fresh peer connection + control channel and send an offer.
    BuildPcAndOffer,
    /// Build a fresh peer connection and answer the given offer.
    BuildPcAndAnswer { sdp: String },
    /// Set the remote description from a received answer.
    SetRemoteAnswer { sdp: String },
    /// Add the most-recently-received ICE candidate.
    AddIce,
    /// Tear down the current peer connection + transfer (stale-connection cleanup).
    TeardownPc,
    /// Set the UI status.
    SetStatus(Status),
    /// Persist `room` to the URL hash + session ownership + share UI.
    PersistRoom { room: String },
}

/// Apply one event to the session, returning the actions to execute in order.
pub fn step(s: &mut Session, ev: Event) -> Vec<Action> {
    match ev {
        Event::Open => match &s.room_in_hash {
            // A reload (or shared link) rejoins the room in the URL first.
            Some(room) => vec![Action::SendJoin { room: room.clone() }],
            // A fresh visit creates a new room.
            None => vec![Action::SendCreate],
        },
        // Remaining variants are added in later tasks.
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_visit_creates_a_room() {
        let mut s = Session::default();
        assert_eq!(step(&mut s, Event::Open), vec![Action::SendCreate]);
    }

    #[test]
    fn reload_with_room_in_hash_rejoins_it() {
        let mut s = Session { room_in_hash: Some("k7m4qp".into()), ..Session::default() };
        assert_eq!(
            step(&mut s, Event::Open),
            vec![Action::SendJoin { room: "k7m4qp".into() }]
        );
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p client reconnect`
Expected: PASS (2 tests: `fresh_visit_creates_a_room`, `reload_with_room_in_hash_rejoins_it`).

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/reconnect.rs crates/client/src/main.rs crates/client/src/ui.rs
git commit -m "client: add reconnect reducer with open-handshake decision"
```

---

## Task 2: Role dynamics — offer/answer/ICE/peer-left

**Files:**
- Modify: `crates/client/src/reconnect.rs`

- [ ] **Step 1: Write failing tests for the role/connection transitions**

Add to the `tests` module in `reconnect.rs`:

```rust
    #[test]
    fn peer_joined_builds_pc_and_offers() {
        let mut s = Session::default();
        assert_eq!(step(&mut s, Event::PeerJoined), vec![Action::BuildPcAndOffer]);
        assert!(s.has_pc, "offering side now has a live pc");
    }

    #[test]
    fn offer_builds_pc_and_answers() {
        let mut s = Session::default();
        assert_eq!(
            step(&mut s, Event::Offer { sdp: "OFFER".into() }),
            vec![Action::BuildPcAndAnswer { sdp: "OFFER".into() }]
        );
        assert!(s.has_pc, "answering side now has a live pc");
    }

    #[test]
    fn answer_sets_remote_only_when_a_pc_exists() {
        let mut s = Session { has_pc: true, ..Session::default() };
        assert_eq!(
            step(&mut s, Event::Answer { sdp: "ANS".into() }),
            vec![Action::SetRemoteAnswer { sdp: "ANS".into() }]
        );
        // Without a pc, a stray answer is ignored (today's "recv answer but no pc").
        let mut empty = Session::default();
        assert_eq!(step(&mut empty, Event::Answer { sdp: "ANS".into() }), vec![]);
    }

    #[test]
    fn ice_added_only_when_a_pc_exists() {
        let mut s = Session { has_pc: true, ..Session::default() };
        assert_eq!(step(&mut s, Event::Ice), vec![Action::AddIce]);
        let mut empty = Session::default();
        assert_eq!(step(&mut empty, Event::Ice), vec![]);
    }

    #[test]
    fn peer_left_tears_down_and_clears_pc() {
        let mut s = Session { has_pc: true, ..Session::default() };
        assert_eq!(
            step(&mut s, Event::PeerLeft),
            vec![Action::TeardownPc, Action::SetStatus(Status::PeerLeft)]
        );
        assert!(!s.has_pc, "pc cleared so a reconnect starts clean");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p client reconnect`
Expected: FAIL — these events currently hit the `_ => vec![]` arm, so e.g. `peer_joined_builds_pc_and_offers` asserts `[BuildPcAndOffer]` but gets `[]`.

- [ ] **Step 3: Implement the transitions**

In `reconnect.rs`, replace the `// Remaining variants...` / `_ => vec![]` arm with:

```rust
        Event::PeerJoined => {
            s.has_pc = true;
            vec![Action::BuildPcAndOffer]
        }
        Event::Offer { sdp } => {
            s.has_pc = true;
            vec![Action::BuildPcAndAnswer { sdp }]
        }
        Event::Answer { sdp } => {
            if s.has_pc {
                vec![Action::SetRemoteAnswer { sdp }]
            } else {
                vec![]
            }
        }
        Event::Ice => {
            if s.has_pc {
                vec![Action::AddIce]
            } else {
                vec![]
            }
        }
        Event::PeerLeft => {
            s.has_pc = false;
            vec![Action::TeardownPc, Action::SetStatus(Status::PeerLeft)]
        }
        // Created / RoomFull / RoomNotFound are added in later tasks.
        _ => vec![],
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p client reconnect`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/reconnect.rs
git commit -m "client: reducer handles offer/answer/ice/peer-left role dynamics"
```

---

## Task 3: Created and RoomFull status transitions

**Files:**
- Modify: `crates/client/src/reconnect.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module:

```rust
    #[test]
    fn created_persists_room_and_waits() {
        let mut s = Session::default();
        assert_eq!(
            step(&mut s, Event::Created { room: "abc23".into() }),
            vec![
                Action::PersistRoom { room: "abc23".into() },
                Action::SetStatus(Status::WaitingForPeer),
            ]
        );
        // The created room is now both owned and the hash room (so a later
        // RoomNotFound can reclaim it).
        assert_eq!(s.owns.as_deref(), Some("abc23"));
        assert_eq!(s.room_in_hash.as_deref(), Some("abc23"));
    }

    #[test]
    fn room_full_sets_status() {
        let mut s = Session::default();
        assert_eq!(
            step(&mut s, Event::RoomFull),
            vec![Action::SetStatus(Status::RoomFull)]
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p client reconnect`
Expected: FAIL — `Created`/`RoomFull` hit `_ => vec![]`.

- [ ] **Step 3: Implement**

In `reconnect.rs`, replace the `// Created / RoomFull / RoomNotFound...` / `_ => vec![]` arm with:

```rust
        Event::Created { room } => {
            s.owns = Some(room.clone());
            s.room_in_hash = Some(room.clone());
            vec![
                Action::PersistRoom { room },
                Action::SetStatus(Status::WaitingForPeer),
            ]
        }
        Event::RoomFull => vec![Action::SetStatus(Status::RoomFull)],
        // RoomNotFound is added in Task 4.
        Event::RoomNotFound => vec![],
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p client reconnect`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/reconnect.rs
git commit -m "client: reducer handles created + room-full"
```

---

## Task 4: Reclaim logic (the security + double-reclaim guards)

**Files:**
- Modify: `crates/client/src/reconnect.rs`

- [ ] **Step 1: Write failing tests covering all three reclaim paths**

Add to the `tests` module:

```rust
    /// Helper: an owner reloading — hash and ownership agree on the same room.
    fn owner_reload(room: &str) -> Session {
        Session { room_in_hash: Some(room.into()), owns: Some(room.into()), ..Session::default() }
    }

    #[test]
    fn owner_reclaims_a_vanished_room_once() {
        let mut s = owner_reload("k7m4qp");
        // Reload tried Join first; the room was torn down -> RoomNotFound.
        assert_eq!(
            step(&mut s, Event::RoomNotFound),
            vec![Action::SendReclaim { room: "k7m4qp".into() }]
        );
        assert!(s.reclaim_tried);
    }

    #[test]
    fn second_room_not_found_does_not_reclaim_again() {
        let mut s = owner_reload("k7m4qp");
        let _ = step(&mut s, Event::RoomNotFound); // first -> reclaim
        // A reclaim that was refused comes back as RoomNotFound; do NOT loop.
        assert_eq!(
            step(&mut s, Event::RoomNotFound),
            vec![Action::SetStatus(Status::RoomNotFound)]
        );
    }

    #[test]
    fn non_owner_never_reclaims() {
        // A joiner has the room in the URL but never owned it.
        let mut s = Session { room_in_hash: Some("k7m4qp".into()), owns: None, ..Session::default() };
        assert_eq!(
            step(&mut s, Event::RoomNotFound),
            vec![Action::SetStatus(Status::RoomNotFound)]
        );
        assert!(!s.reclaim_tried, "a non-owner must not attempt a reclaim");
    }

    #[test]
    fn owner_of_a_different_room_does_not_reclaim() {
        // Hash and ownership disagree -> not the owner of *this* room.
        let mut s = Session {
            room_in_hash: Some("k7m4qp".into()),
            owns: Some("other9".into()),
            ..Session::default()
        };
        assert_eq!(
            step(&mut s, Event::RoomNotFound),
            vec![Action::SetStatus(Status::RoomNotFound)]
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p client reconnect`
Expected: FAIL — `RoomNotFound` currently returns `vec![]`.

- [ ] **Step 3: Implement the reclaim decision**

In `reconnect.rs`, replace `Event::RoomNotFound => vec![],` with:

```rust
        Event::RoomNotFound => {
            // Only the tab that created this exact room may reclaim it, and only
            // once — so a refused reclaim (which returns RoomNotFound again)
            // cannot loop, and a guessed code cannot trigger a reclaim.
            let is_owner = matches!(
                (&s.room_in_hash, &s.owns),
                (Some(h), Some(o)) if h == o
            );
            if is_owner && !s.reclaim_tried {
                s.reclaim_tried = true;
                let room = s.room_in_hash.clone().expect("owner has a hash room");
                vec![Action::SendReclaim { room }]
            } else {
                vec![Action::SetStatus(Status::RoomNotFound)]
            }
        }
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p client reconnect`
Expected: PASS (13 tests).

- [ ] **Step 5: Run the whole client test suite to confirm nothing regressed**

Run: `cargo test -p client`
Expected: PASS (all existing tests + the 13 reducer tests).

- [ ] **Step 6: Commit**

```bash
git add crates/client/src/reconnect.rs
git commit -m "client: reducer reclaim logic with owner + once-only guards"
```

---

## Task 5: Wire the reducer into `app.rs`

This refactor is mechanical: the inbound-message callback becomes translate→`step`→execute, and the `on_open` Create/Join decision moves into the reducer (the socket simply feeds `Event::Open`). Behavior is preserved — the reducer encodes exactly the decisions Tasks 1–4 characterized from the current code. There is no new unit test here; correctness is verified by build + the existing suite, and exercised end-to-end by Plan 2's Playwright suite.

**Files:**
- Modify: `crates/client/src/app.rs`

- [ ] **Step 1: Add the reducer import and a shared `Session`**

In `crates/client/src/app.rs`, add to the imports near the top (after `use crate::qr::qr_svg;`):

```rust
use crate::reconnect::{step, Action, Event, Session};
```

Inside `App()` (after the existing `let pending: Rc<RefCell<Vec<...>>>` on line ~149), seed a session from the live hash + ownership:

```rust
    // Reconnect decisions live in the pure `reconnect` reducer; this is the
    // mutable session it reads/maintains, seeded from the URL + session storage.
    let session: Rc<RefCell<Session>> = Rc::new(RefCell::new(Session {
        room_in_hash: room_from_hash(),
        owns: session_owns(),
        reclaim_tried: false,
        has_pc: false,
    }));
```

- [ ] **Step 2: Add an `execute` helper that performs one `Action`**

The existing closures (`build_pc`, `wire_ctrl`, `set_*` signals, `sig`, `pc`, `transfer`) are all in scope inside the mount `Effect`. Replace the body of the inbound-message callback (the `move |msg| match msg { ... }` passed to `Signaling::connect`, currently `app.rs:287-414`) with a translate-then-execute body. Replace from `move |msg| match msg {` through its closing `}` (the line before the `}) {` that ends the `Signaling::connect(` call) with:

```rust
                move |msg| {
                    // Translate the wire message into a reducer event. Messages
                    // with no handshake decision (server-originated echoes) map to
                    // nothing and are ignored.
                    let ev = match msg {
                        SignalMsg::Created { room } => Some(Event::Created { room }),
                        SignalMsg::PeerJoined => Some(Event::PeerJoined),
                        SignalMsg::Sdp { sdp, kind } if kind == "offer" => {
                            Some(Event::Offer { sdp })
                        }
                        SignalMsg::Sdp { sdp, .. } => Some(Event::Answer { sdp }),
                        SignalMsg::Ice { candidate } => {
                            // Stash the candidate for the AddIce action to consume.
                            *last_ice.borrow_mut() = Some(candidate);
                            Some(Event::Ice)
                        }
                        SignalMsg::PeerLeft => Some(Event::PeerLeft),
                        SignalMsg::RoomFull => Some(Event::RoomFull),
                        SignalMsg::RoomNotFound => Some(Event::RoomNotFound),
                        _ => None,
                    };
                    let Some(ev) = ev else { return };
                    let actions = step(&mut session.borrow_mut(), ev);
                    for action in actions {
                        execute(action);
                    }
                }
```

- [ ] **Step 3: Define `last_ice` and the `execute` closure**

Inbound ICE carries the candidate string, but `Event::Ice` is payload-free; the candidate is stashed in `last_ice` and consumed by `Action::AddIce`. Immediately before the `let signaling = match Signaling::connect({` line (`app.rs:280`), add:

```rust
            // Holds the candidate from the most recent inbound `Ice` message so the
            // `AddIce` action can apply it (the reducer event is payload-free).
            let last_ice: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

            // Execute one reducer action against the browser/Leptos world. This is
            // the only place handshake side effects happen.
            let execute: Rc<dyn Fn(Action)> = {
                let build_pc = build_pc.clone();
                let pc = pc.clone();
                let sig_exec = sig.clone();
                let wire_ctrl = wire_ctrl.clone();
                let transfer = transfer.clone();
                let last_ice = last_ice.clone();
                Rc::new(move |action: Action| match action {
                    Action::SendCreate => {
                        if let Some(s) = sig_exec.borrow().as_ref() {
                            s.send(&SignalMsg::Create);
                        }
                    }
                    Action::SendJoin { room } => {
                        if let Some(s) = sig_exec.borrow().as_ref() {
                            s.send(&SignalMsg::Join { room });
                        }
                    }
                    Action::SendReclaim { room } => {
                        if let Some(s) = sig_exec.borrow().as_ref() {
                            s.send(&SignalMsg::Reclaim { room });
                        }
                    }
                    Action::BuildPcAndOffer => {
                        crate::log::clog("[rtc] PeerJoined -> building pc + offering");
                        let Some(peer) = build_pc() else {
                            crate::log::clog("[rtc] PeerJoined: build_pc failed");
                            return;
                        };
                        let channel =
                            webrtc::create_data_channel(&peer, crate::transfer::CTRL_LABEL);
                        wire_ctrl(peer.clone(), channel);
                        let sig_for_cb = sig_exec.clone();
                        spawn_local(async move {
                            match webrtc::create_offer(&peer).await {
                                Ok(sdp) => {
                                    if let Some(s) = sig_for_cb.borrow().as_ref() {
                                        s.send(&SignalMsg::Sdp { sdp, kind: "offer".into() });
                                        crate::log::clog("[rtc] sent offer");
                                    }
                                }
                                Err(e) => crate::log::clog_val("[rtc] create_offer ERR", &e),
                            }
                        });
                    }
                    Action::BuildPcAndAnswer { sdp } => {
                        crate::log::clog("[rtc] recv offer -> building pc + answering");
                        let Some(peer) = build_pc() else {
                            crate::log::clog("[rtc] recv offer: build_pc failed");
                            return;
                        };
                        let sig_for_cb = sig_exec.clone();
                        spawn_local(async move {
                            if let Err(e) = webrtc::set_remote(&peer, RtcSdpType::Offer, &sdp).await {
                                crate::log::clog_val("[rtc] set_remote(offer) ERR", &e);
                                return;
                            }
                            crate::log::clog("[rtc] set_remote(offer) ok");
                            match webrtc::create_answer(&peer).await {
                                Ok(answer) => {
                                    if let Some(s) = sig_for_cb.borrow().as_ref() {
                                        s.send(&SignalMsg::Sdp { sdp: answer, kind: "answer".into() });
                                        crate::log::clog("[rtc] sent answer");
                                    } else {
                                        crate::log::clog("[rtc] no signaling to send answer");
                                    }
                                }
                                Err(e) => crate::log::clog_val("[rtc] create_answer ERR", &e),
                            }
                        });
                    }
                    Action::SetRemoteAnswer { sdp } => {
                        crate::log::clog("[rtc] recv answer -> set_remote");
                        if let Some(peer) = pc.borrow().clone() {
                            spawn_local(async move {
                                match webrtc::set_remote(&peer, RtcSdpType::Answer, &sdp).await {
                                    Ok(_) => crate::log::clog("[rtc] set_remote(answer) ok"),
                                    Err(e) => crate::log::clog_val("[rtc] set_remote(answer) ERR", &e),
                                }
                            });
                        } else {
                            crate::log::clog("[rtc] recv answer but no pc");
                        }
                    }
                    Action::AddIce => {
                        let Some(candidate) = last_ice.borrow_mut().take() else { return };
                        if let Some(peer) = pc.borrow().clone() {
                            spawn_local(async move {
                                if let Err(e) = webrtc::add_ice_candidate(&peer, &candidate).await {
                                    crate::log::clog_val("[rtc] add_ice_candidate ERR", &e);
                                }
                            });
                        } else {
                            crate::log::clog("[rtc] recv ICE but no pc yet");
                        }
                    }
                    Action::TeardownPc => {
                        crate::log::clog("[rtc] PeerLeft -> closing pc");
                        if let Some(peer) = pc.borrow_mut().take() {
                            peer.close();
                        }
                        *transfer.borrow_mut() = None;
                    }
                    Action::SetStatus(s) => set_status.set(s),
                    Action::PersistRoom { room } => {
                        let link = format!("{}/#/room/{room}", public_origin());
                        set_qr.set(qr_svg(&link));
                        set_room_link.set(link);
                        let _ = web_sys::window()
                            .unwrap()
                            .location()
                            .set_hash(&format!("/room/{room}"));
                        set_session_owns(&room);
                        set_room_code.set(room);
                    }
                })
            };
```

Note: the old code reset `reclaim_tried` via a `Cell`; that bookkeeping now lives in `Session`, so delete the old `let reclaim_tried = Rc::new(Cell::new(false));` line (`app.rs:274`) and its `.clone()` capture inside the callback.

- [ ] **Step 4: Feed `Event::Open` from the socket open**

Replace the existing `on_open` block (`app.rs:427-431`) — which decided Create-vs-Join itself — with a version that just drives the reducer:

```rust
            // On open, let the reducer decide Create vs (re)Join from the session.
            let session_open = session.clone();
            let execute_open = execute.clone();
            signaling.on_open(move || {
                let actions = step(&mut session_open.borrow_mut(), Event::Open);
                for action in actions {
                    execute_open(action);
                }
            });
```

- [ ] **Step 5: Build the client (native) and run the suite**

Run: `cargo test -p client`
Expected: PASS (existing tests + 13 reducer tests; no regressions).

- [ ] **Step 6: Build the WASM client to confirm it still compiles for the browser target**

Run: `cd crates/client && trunk build && cd ../..`
Expected: build succeeds (no errors). If `trunk` is unavailable, run `cargo build -p client --target wasm32-unknown-unknown` instead.

- [ ] **Step 7: Commit**

```bash
git add crates/client/src/app.rs
git commit -m "client: drive signaling handshake through the reconnect reducer"
```

---

## Task 6: Marked-failing file-id validation seam

The receiver never checks that `FileEnd.id` matches the file it has open (`transfer_state::finalize_decision` ignores any id). This is the documented file-id gap. Add the seam and an `#[ignore]`'d test that pins the desired behavior; it fails today and flips green when validation lands.

**Files:**
- Modify: `crates/client/src/transfer_state.rs`

- [ ] **Step 1: Add the seam (stubbed to current, unvalidated behavior)**

In `crates/client/src/transfer_state.rs`, after `finalize_decision` (line ~98), add:

```rust
/// Receiver: consume the incoming state at end-of-file, validating that the
/// `FileEnd.id` matches the file currently open. Returns the meta to save only
/// on a match; a mismatch means a protocol desync (e.g. a future bidirectional/
/// parallel mode crossing id spaces) and must NOT save bytes under the wrong id.
///
/// NOTE (known gap): this currently ignores `end_id` and behaves like
/// `finalize_decision`. The `#[ignore]`d test `finalize_rejects_mismatched_id`
/// pins the target behavior; implement the id check to make it pass.
#[allow(dead_code)]
pub(crate) fn finalize_decision_checked(inc: &mut Incoming, _end_id: u64) -> Option<FileStart> {
    inc.meta.take()
}
```

- [ ] **Step 2: Add the marked-failing test**

In the `tests` module of `transfer_state.rs`, add:

```rust
    #[test]
    #[ignore = "known gap: receiver does not validate FileEnd.id against the open file"]
    fn finalize_rejects_mismatched_id() {
        let mut inc = incoming_with(meta(7));
        // A FileEnd for a different id must not finalize this file.
        assert!(finalize_decision_checked(&mut inc, 999).is_none());
        // The open file's meta must remain available for a correct End.
        assert!(inc.meta.is_some());
    }

    #[test]
    fn finalize_checked_accepts_matching_id() {
        let mut inc = incoming_with(meta(7));
        let m = finalize_decision_checked(&mut inc, 7).unwrap();
        assert_eq!(m.id, 7);
    }
```

- [ ] **Step 3: Verify the matching-id test passes and the marked one is skipped**

Run: `cargo test -p client transfer_state`
Expected: PASS for `finalize_checked_accepts_matching_id`; `finalize_rejects_mismatched_id` shows as `ignored`.

- [ ] **Step 4: Verify the marked test actually fails today (the crack is real)**

Run: `cargo test -p client transfer_state -- --ignored`
Expected: FAIL on `finalize_rejects_mismatched_id` (the stub returns `Some` for id 999). This confirms it is a live backlog item, not a no-op.

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/transfer_state.rs
git commit -m "client: add file-id validation seam + marked-failing test"
```

---

## Task 7: Marked-failing room-TTL seam

Idle single-peer rooms never expire (no timer exists). Add a pure `reap_idle` seam on `RoomRegistry` plus activity tracking, and an `#[ignore]`'d test that pins idle expiry. The seam takes `now_ms` + `ttl_ms` as parameters (no wall clock), so the eventual reaper task and the test both drive it deterministically — this is the config seam the spec requires.

**Files:**
- Modify: `crates/server/src/rooms.rs`

- [ ] **Step 1: Track last-activity per room**

In `crates/server/src/rooms.rs`, change the `Room` struct (line ~20) to carry an activity timestamp:

```rust
#[derive(Default)]
struct Room {
    peers: Vec<PeerId>,
    /// Last time a peer joined/created this room (ms since epoch). Updated by the
    /// caller via `touch`; used by `reap_idle` to expire idle single-peer rooms.
    last_activity_ms: u64,
}
```

- [ ] **Step 2: Add `touch` and the stubbed `reap_idle` seam**

In `rooms.rs`, add these methods to the `impl RoomRegistry` block (after `contains`, line ~62):

```rust
    /// Record activity on `room` at `now_ms` (resets its idle clock).
    #[allow(dead_code)]
    pub fn touch(&mut self, room_id: &str, now_ms: u64) {
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.last_activity_ms = now_ms;
        }
    }

    /// Remove rooms that have sat idle (a single waiting peer, no activity) for at
    /// least `ttl_ms`, returning the ids removed so the caller can notify/clean up.
    ///
    /// NOTE (known gap): not yet implemented — returns nothing, so idle rooms never
    /// expire. The `#[ignore]`d test `reap_idle_expires_lonely_room` pins the
    /// target; implement this (and call it from a server timer) to make it pass.
    #[allow(dead_code)]
    pub fn reap_idle(&mut self, _now_ms: u64, _ttl_ms: u64) -> Vec<String> {
        Vec::new()
    }
```

- [ ] **Step 3: Set initial activity on create/join**

So a future implementation has data to act on (and the test's setup is realistic), stamp activity when peers enter. In `create` (line ~39), change the inserted room to record the creation time — update the signature to take `now_ms`:

```rust
    /// Create a room with a caller-supplied id and place `peer` in it.
    pub fn create(&mut self, peer: PeerId, room_id: String, now_ms: u64) -> JoinOutcome {
        self.rooms.insert(room_id.clone(), Room { peers: vec![peer], last_activity_ms: now_ms });
        self.peer_room.insert(peer, room_id.clone());
        JoinOutcome::Created(room_id)
    }
```

And in `join` (the `Some(room)` arm, line ~50), stamp it too:

```rust
            Some(room) => {
                room.peers.push(peer);
                room.last_activity_ms = now_ms;
                self.peer_room.insert(peer, room_id.to_string());
                JoinOutcome::Joined
            }
```

Update `join`'s signature to take `now_ms`:

```rust
    pub fn join(&mut self, peer: PeerId, room_id: &str, now_ms: u64) -> JoinOutcome {
```

- [ ] **Step 4: Update existing callers and tests to pass `now_ms`**

Callers in `crates/server/src/ws.rs`: `create` (line ~175) and `join` (line ~216). Update them to pass the existing `now_ms()` / `now` value:

In the `SignalMsg::Create` arm:

```rust
            let outcome = state.registry.lock().unwrap().create(peer, room_id, now_ms());
```

In the `SignalMsg::Reclaim` arm's create (line ~202):

```rust
            let outcome = state.registry.lock().unwrap().create(peer, room, now_ms());
```

In the `SignalMsg::Join` arm (line ~216), `now` is already bound earlier in that arm:

```rust
            let outcome = state.registry.lock().unwrap().join(peer, &room, now);
```

Then fix the existing `rooms.rs` unit tests (lines ~104-143) to pass a timestamp — they call `create`/`join` without one. Use `0` for these (they don't exercise time):

```rust
    #[test]
    fn create_then_join_pairs_peers() {
        let mut r = RoomRegistry::new();
        assert_eq!(r.create(1, "room1".into(), 0), JoinOutcome::Created("room1".into()));
        assert_eq!(r.join(2, "room1", 0), JoinOutcome::Joined);
        assert_eq!(r.partner(1), Some(2));
        assert_eq!(r.partner(2), Some(1));
    }

    #[test]
    fn third_peer_is_full() {
        let mut r = RoomRegistry::new();
        r.create(1, "room1".into(), 0);
        r.join(2, "room1", 0);
        assert_eq!(r.join(3, "room1", 0), JoinOutcome::Full);
    }

    #[test]
    fn join_unknown_room_not_found() {
        let mut r = RoomRegistry::new();
        assert_eq!(r.join(1, "nope", 0), JoinOutcome::NotFound);
    }

    #[test]
    fn remove_notifies_partner_and_keeps_room() {
        let mut r = RoomRegistry::new();
        r.create(1, "room1".into(), 0);
        r.join(2, "room1", 0);
        assert_eq!(r.remove(1), Some(2));
        assert_eq!(r.partner(2), None);
        assert_eq!(r.room_count(), 1);
    }

    #[test]
    fn remove_last_peer_drops_room() {
        let mut r = RoomRegistry::new();
        r.create(1, "room1".into(), 0);
        assert_eq!(r.remove(1), None);
        assert_eq!(r.room_count(), 0);
    }
```

Also update the `ws.rs` unit tests that call `handle_message` — those go through `handle_message`, not the registry directly, so they need no change. But the two `handle_message` arms now pass `now_ms()`; that compiles without test edits.

- [ ] **Step 5: Add the marked-failing TTL test**

In the `tests` module of `rooms.rs`, add:

```rust
    #[test]
    #[ignore = "known gap: no room TTL — idle single-peer rooms never expire"]
    fn reap_idle_expires_lonely_room() {
        let mut r = RoomRegistry::new();
        // A peer creates a room at t=0 and is left waiting alone.
        r.create(1, "lonely".into(), 0);
        // 60s later, with a 30s TTL, the idle room should be reaped.
        let reaped = r.reap_idle(60_000, 30_000);
        assert_eq!(reaped, vec!["lonely".to_string()]);
        assert!(!r.contains("lonely"), "expired room must be gone");
    }

    #[test]
    fn touch_updates_activity_without_panicking_on_unknown_room() {
        let mut r = RoomRegistry::new();
        r.create(1, "live".into(), 0);
        r.touch("live", 5_000);
        r.touch("ghost", 5_000); // no-op, must not panic
        assert!(r.contains("live"));
    }
```

- [ ] **Step 6: Verify the suite passes and the marked test is skipped, then confirm it fails under `--ignored`**

Run: `cargo test -p server`
Expected: PASS (all server tests; `reap_idle_expires_lonely_room` shows `ignored`).

Run: `cargo test -p server -- --ignored`
Expected: FAIL on `reap_idle_expires_lonely_room` (stub returns empty → room still present). Confirms the live backlog item.

- [ ] **Step 7: Commit**

```bash
git add crates/server/src/rooms.rs crates/server/src/ws.rs
git commit -m "server: add room-TTL reap_idle seam + marked-failing test"
```

---

## Task 8: CI — non-blocking marked-failing backlog job

**Files:**
- Modify: `.github/workflows/rust.yml`

- [ ] **Step 1: Add the `ignored-backlog` job**

In `.github/workflows/rust.yml`, add a new job under `jobs:` (sibling to `build` and `docker`), after the `build` job:

```yaml
  ignored-backlog:
    # The marked-failing "crack-finder" tests: known gaps with no fix yet.
    # Non-blocking — a failure here is expected and informational. When one of
    # these starts PASSING, the underlying bug is fixed and its #[ignore] should
    # be removed so it joins the required `build` gate.
    runs-on: ubuntu-latest
    continue-on-error: true
    steps:
    - uses: actions/checkout@v6
    - name: Run marked-failing backlog
      run: cargo test --verbose -- --ignored
```

- [ ] **Step 2: Validate the workflow YAML locally**

Run: `cargo test --verbose -- --ignored`
Expected: the two marked tests run and FAIL (`finalize_rejects_mismatched_id`, `reap_idle_expires_lonely_room`) — this is the expected backlog signal. (The job is `continue-on-error`, so CI stays green overall.)

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/rust.yml
git commit -m "ci: add non-blocking marked-failing backlog job"
```

---

## Self-review

**Spec coverage (Plan 1 portion):**
- Reducer extraction (`reconnect.rs`) + native unit tests for sleep/reconnect decision logic, double-reclaim guard, hijack guard, role dynamics, teardown → Tasks 1–5. ✓
- `app.rs` shrinks to translate→step→execute → Task 5. ✓
- Characterization-first / behavior-preserving → Tasks 1–4 encode current behavior before Task 5 moves it. ✓
- file-id marked-failing test → Task 6. ✓
- Room TTL marked-failing test + config (param) seam → Task 7. ✓
- CI non-blocking `--ignored` job → Task 8. ✓
- Server reclaim/rate-limit/ICE "passing" tests → already exist in `ws.rs`; deliberately not duplicated (noted in header). ✓
- Out of this plan (own plan): Playwright E2E, `docs/testing.md` index → `2026-06-03-testing-strategy-e2e.md`.

**Placeholder scan:** No TBD/TODO-as-work; the two "NOTE (known gap)" comments are intentional seam documentation, each backed by an `#[ignore]` test with a concrete target. All code steps show complete code.

**Type consistency:** `Event`, `Session`, `Action`, `step` signatures match across Tasks 1–5. `Status` gains `Debug` (Task 1) before `Action` (which contains `Status`) is asserted with `assert_eq!`. `create`/`join` signature change (Task 7) is propagated to every caller (ws.rs arms) and every existing test in the same task. `finalize_decision_checked(&mut Incoming, u64) -> Option<FileStart>` matches its tests.

**Known follow-up not owned here:** when room TTL is implemented, a server timer must call `reap_idle` and `touch` must be wired on relay activity; Task 7 only lands the seam + failing test, by design.
