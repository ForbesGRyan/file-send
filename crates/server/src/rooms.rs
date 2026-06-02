//! In-memory room registry. Pure bookkeeping; networking lives in `ws.rs`.

use std::collections::HashMap;

pub type PeerId = u64;

/// Outcome of attempting to place a peer into a room.
#[derive(Debug, PartialEq)]
pub enum JoinOutcome {
    /// Peer created and now owns a new room (is the initiator).
    Created(String),
    /// Peer joined an existing room as the second member.
    Joined,
    /// Room exists but already has two peers.
    Full,
    /// No room with that id.
    NotFound,
}

#[derive(Default)]
struct Room {
    peers: Vec<PeerId>,
}

/// Tracks rooms and their (at most two) peers.
#[derive(Default)]
pub struct RoomRegistry {
    rooms: HashMap<String, Room>,
    /// Reverse index: which room a peer is in.
    peer_room: HashMap<PeerId, String>,
}

impl RoomRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a room with a caller-supplied id and place `peer` in it.
    pub fn create(&mut self, peer: PeerId, room_id: String) -> JoinOutcome {
        self.rooms.insert(room_id.clone(), Room { peers: vec![peer] });
        self.peer_room.insert(peer, room_id.clone());
        JoinOutcome::Created(room_id)
    }

    /// Place `peer` into an existing room.
    pub fn join(&mut self, peer: PeerId, room_id: &str) -> JoinOutcome {
        match self.rooms.get_mut(room_id) {
            None => JoinOutcome::NotFound,
            Some(room) if room.peers.len() >= 2 => JoinOutcome::Full,
            Some(room) => {
                room.peers.push(peer);
                self.peer_room.insert(peer, room_id.to_string());
                JoinOutcome::Joined
            }
        }
    }

    /// Does a live room with this id exist? (Rooms are dropped when empty, so this
    /// is true only while some peer holds it.)
    pub fn contains(&self, room_id: &str) -> bool {
        self.rooms.contains_key(room_id)
    }

    /// Return the other peer in `peer`'s room, if any.
    pub fn partner(&self, peer: PeerId) -> Option<PeerId> {
        let room_id = self.peer_room.get(&peer)?;
        let room = self.rooms.get(room_id)?;
        room.peers.iter().copied().find(|&p| p != peer)
    }

    /// Remove `peer`; returns the partner that should be notified (if any).
    /// Drops the room entirely once empty.
    pub fn remove(&mut self, peer: PeerId) -> Option<PeerId> {
        let room_id = self.peer_room.remove(&peer)?;
        let partner = {
            let room = self.rooms.get_mut(&room_id)?;
            room.peers.retain(|&p| p != peer);
            room.peers.first().copied()
        };
        if self.rooms.get(&room_id).map(|r| r.peers.is_empty()).unwrap_or(false) {
            self.rooms.remove(&room_id);
        }
        partner
    }

    #[cfg(test)]
    fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Debug snapshot: the room a peer is in and that room's current members.
    /// Used only by diagnostic logging in `ws.rs`.
    pub fn debug_room_of(&self, peer: PeerId) -> Option<(String, Vec<PeerId>)> {
        let room_id = self.peer_room.get(&peer)?;
        let members = self.rooms.get(room_id)?.peers.clone();
        Some((room_id.clone(), members))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_join_pairs_peers() {
        let mut r = RoomRegistry::new();
        assert_eq!(r.create(1, "room1".into()), JoinOutcome::Created("room1".into()));
        assert_eq!(r.join(2, "room1"), JoinOutcome::Joined);
        assert_eq!(r.partner(1), Some(2));
        assert_eq!(r.partner(2), Some(1));
    }

    #[test]
    fn third_peer_is_full() {
        let mut r = RoomRegistry::new();
        r.create(1, "room1".into());
        r.join(2, "room1");
        assert_eq!(r.join(3, "room1"), JoinOutcome::Full);
    }

    #[test]
    fn join_unknown_room_not_found() {
        let mut r = RoomRegistry::new();
        assert_eq!(r.join(1, "nope"), JoinOutcome::NotFound);
    }

    #[test]
    fn remove_notifies_partner_and_keeps_room() {
        let mut r = RoomRegistry::new();
        r.create(1, "room1".into());
        r.join(2, "room1");
        assert_eq!(r.remove(1), Some(2));
        assert_eq!(r.partner(2), None);
        assert_eq!(r.room_count(), 1);
    }

    #[test]
    fn remove_last_peer_drops_room() {
        let mut r = RoomRegistry::new();
        r.create(1, "room1".into());
        assert_eq!(r.remove(1), None);
        assert_eq!(r.room_count(), 0);
    }
}
