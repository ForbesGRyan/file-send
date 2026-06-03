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
    /// Last time a peer joined/created this room (ms since epoch). Updated by the
    /// caller via `touch`; used by `reap_idle` to expire idle single-peer rooms.
    last_activity_ms: u64,
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
    pub fn create(&mut self, peer: PeerId, room_id: String, now_ms: u64) -> JoinOutcome {
        self.rooms.insert(room_id.clone(), Room { peers: vec![peer], last_activity_ms: now_ms });
        self.peer_room.insert(peer, room_id.clone());
        JoinOutcome::Created(room_id)
    }

    /// Place `peer` into an existing room.
    pub fn join(&mut self, peer: PeerId, room_id: &str, now_ms: u64) -> JoinOutcome {
        match self.rooms.get_mut(room_id) {
            None => JoinOutcome::NotFound,
            Some(room) if room.peers.len() >= 2 => JoinOutcome::Full,
            Some(room) => {
                room.peers.push(peer);
                room.last_activity_ms = now_ms;
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

    /// Record activity on `room` at `now_ms` (resets its idle clock).
    #[allow(dead_code)]
    pub fn touch(&mut self, room_id: &str, now_ms: u64) {
        if let Some(room) = self.rooms.get_mut(room_id) {
            room.last_activity_ms = now_ms;
        }
    }

    /// Remove rooms that have sat idle (a single waiting peer, no activity) for at
    /// least `ttl_ms`, returning the ids removed so the caller can notify/clean up.
    /// Rooms with two peers are active and are never reaped — only single-peer
    /// idle rooms expire.
    ///
    /// NOTE (known gap): not yet implemented — returns nothing, so idle rooms never
    /// expire. The `#[ignore]`d test `reap_idle_expires_lonely_room` pins the
    /// target; implement this (and call it from a server timer) to make it pass.
    #[allow(dead_code)]
    pub fn reap_idle(&mut self, _now_ms: u64, _ttl_ms: u64) -> Vec<String> {
        Vec::new()
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

    /// Test-only: read a room's recorded last-activity timestamp.
    #[cfg(test)]
    fn last_activity_of(&self, room_id: &str) -> Option<u64> {
        self.rooms.get(room_id).map(|r| r.last_activity_ms)
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

    #[test]
    #[ignore = "known gap: no room TTL — idle single-peer rooms never expire; implement reap_idle"]
    fn reap_idle_expires_lonely_room() {
        let mut r = RoomRegistry::new();
        // A peer creates a room at t=0 and is left waiting alone.
        r.create(1, "lonely".into(), 0);
        // A separate room with two peers is active and must survive.
        r.create(2, "paired".into(), 0);
        r.join(3, "paired", 0);
        // 60s later, with a 30s TTL, the idle single-peer room should be reaped,
        // but the paired room must not be.
        let reaped = r.reap_idle(60_000, 30_000);
        assert_eq!(reaped, vec!["lonely".to_string()]);
        assert!(!r.contains("lonely"), "expired room must be gone");
        assert!(r.contains("paired"), "active paired room must survive");
        assert!(!reaped.contains(&"paired".to_string()), "paired room must not be reaped");
    }

    #[test]
    fn touch_updates_activity_without_panicking_on_unknown_room() {
        let mut r = RoomRegistry::new();
        r.create(1, "live".into(), 0);
        r.touch("live", 5_000);
        r.touch("ghost", 5_000); // no-op, must not panic
        assert!(r.contains("live"));
        assert_eq!(r.last_activity_of("live"), Some(5_000), "touch must record the timestamp");
        assert_eq!(r.last_activity_of("ghost"), None, "touching a missing room creates nothing");
    }
}
