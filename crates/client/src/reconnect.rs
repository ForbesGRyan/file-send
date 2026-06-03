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
