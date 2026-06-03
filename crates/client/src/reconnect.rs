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
    ///
    /// The candidate string is not carried here; `app.rs` stashes it before
    /// calling `step`, and `Action::AddIce` consumes it from that stash. This
    /// keeps the reducer payload-free for events that carry no routing decision.
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
}
