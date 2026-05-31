//! WebSocket signaling: one connection per peer, relays messages to partner.

use std::net::{IpAddr, SocketAddr};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    response::Response,
};
use shared::SignalMsg;
use tokio::sync::mpsc;

use crate::limiter::JoinLimiter;
use crate::rooms::{JoinOutcome, PeerId, RoomRegistry};

/// Sliding window for the per-IP join-attempt rate limit.
const JOIN_WINDOW_MS: u64 = 60_000;
/// Max failed join attempts per IP within `JOIN_WINDOW_MS`. Generous for humans
/// (who typically attempt once) but throttles online enumeration of room codes
/// to a rate that makes brute force infeasible.
const JOIN_MAX_FAILURES: usize = 30;

/// Current wall-clock time in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Shared application state, cloned cheaply (Arc) into every handler.
#[derive(Clone)]
pub struct AppState {
    registry: Arc<Mutex<RoomRegistry>>,
    /// Per-peer outbound channel senders, so a peer can push to its partner.
    senders: Arc<Mutex<std::collections::HashMap<PeerId, mpsc::UnboundedSender<SignalMsg>>>>,
    next_id: Arc<AtomicU64>,
    /// Per-IP rate limiter for room-join attempts.
    join_limiter: Arc<Mutex<JoinLimiter>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(RoomRegistry::new())),
            senders: Arc::new(Mutex::new(std::collections::HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            join_limiter: Arc::new(Mutex::new(JoinLimiter::new(JOIN_WINDOW_MS, JOIN_MAX_FAILURES))),
        }
    }

    /// Send a message to a specific peer's outbound channel, if present.
    fn send_to(&self, peer: PeerId, msg: SignalMsg) {
        if let Some(tx) = self.senders.lock().unwrap().get(&peer) {
            let _ = tx.send(msg);
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Axum handler for `GET /ws`: upgrades to a WebSocket.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    let ip = addr.ip();
    ws.on_upgrade(move |socket| handle_socket(socket, state, ip))
}

async fn handle_socket(socket: WebSocket, state: AppState, ip: IpAddr) {
    use futures_util::{SinkExt, StreamExt};

    let peer_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<SignalMsg>();
    state.senders.lock().unwrap().insert(peer_id, tx);

    // Task: drain this peer's outbound channel to the socket.
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = serde_json::to_string(&msg).unwrap();
            if sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Task: read inbound messages and act on them.
    let recv_state = state.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            if let Message::Text(text) = msg {
                let Ok(parsed) = serde_json::from_str::<SignalMsg>(&text) else {
                    continue;
                };
                handle_message(&recv_state, peer_id, ip, parsed);
            } else if let Message::Close(_) = msg {
                break;
            }
        }
    });

    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    }

    // Cleanup on disconnect: notify partner, drop from registry + senders.
    let partner = state.registry.lock().unwrap().remove(peer_id);
    if let Some(partner) = partner {
        state.send_to(partner, SignalMsg::PeerLeft);
    }
    state.senders.lock().unwrap().remove(&peer_id);
}

/// Room-code alphabet: lowercase letters and digits with visually ambiguous
/// characters removed (no `i`, `l`, `o`, `0`, `1`). 31 symbols.
const ROOM_ALPHABET: [char; 31] = [
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'j', 'k', 'm', 'n', 'p', 'q', 'r', 's', 't', 'u', 'v',
    'w', 'x', 'y', 'z', '2', '3', '4', '5', '6', '7', '8', '9',
];

/// Generate a short, easy-to-type room code: 6 chars from `ROOM_ALPHABET`.
fn gen_room_id() -> String {
    nanoid::nanoid!(6, &ROOM_ALPHABET)
}

/// Apply one inbound signaling message. `ip` is the peer's source address, used
/// to rate-limit join attempts.
fn handle_message(state: &AppState, peer: PeerId, ip: IpAddr, msg: SignalMsg) {
    match msg {
        SignalMsg::Create => {
            let room_id = gen_room_id();
            let outcome = state.registry.lock().unwrap().create(peer, room_id);
            if let JoinOutcome::Created(room) = outcome {
                state.send_to(peer, SignalMsg::Created { room });
            }
        }
        SignalMsg::Join { room } => {
            let now = now_ms();
            // Throttle enumeration: an IP over its recent failed-attempt budget
            // is told the room doesn't exist, without touching the registry.
            if !state.join_limiter.lock().unwrap().allowed(ip, now) {
                state.send_to(peer, SignalMsg::RoomNotFound);
                return;
            }
            let outcome = state.registry.lock().unwrap().join(peer, &room);
            match outcome {
                JoinOutcome::Joined => {
                    // Tell the initiator (the partner) to start the WebRTC offer.
                    if let Some(partner) = state.registry.lock().unwrap().partner(peer) {
                        state.send_to(partner, SignalMsg::PeerJoined);
                    }
                }
                JoinOutcome::Full => {
                    state.join_limiter.lock().unwrap().record_failure(ip, now);
                    state.send_to(peer, SignalMsg::RoomFull);
                }
                JoinOutcome::NotFound => {
                    state.join_limiter.lock().unwrap().record_failure(ip, now);
                    state.send_to(peer, SignalMsg::RoomNotFound);
                }
                JoinOutcome::Created(_) => {}
            }
        }
        // Relay SDP and ICE verbatim to the partner.
        relay @ (SignalMsg::Sdp { .. } | SignalMsg::Ice { .. }) => {
            let partner = state.registry.lock().unwrap().partner(peer);
            if let Some(partner) = partner {
                state.send_to(partner, relay);
            }
        }
        // Server-originated variants are ignored if received from a client.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    /// Register an outbound channel for `peer` and return its receiver so tests
    /// can observe what `handle_message` pushes to that peer.
    fn register(state: &AppState, peer: PeerId) -> mpsc::UnboundedReceiver<SignalMsg> {
        let (tx, rx) = mpsc::unbounded_channel();
        state.senders.lock().unwrap().insert(peer, tx);
        rx
    }

    #[test]
    fn room_id_is_six_chars_from_alphabet() {
        for _ in 0..100 {
            let id = gen_room_id();
            assert_eq!(id.chars().count(), 6, "id {id:?} should be 6 chars");
            assert!(
                id.chars().all(|c| ROOM_ALPHABET.contains(&c)),
                "id {id:?} contains a char outside the alphabet"
            );
        }
    }

    #[test]
    fn create_emits_created_with_room_code() {
        let state = AppState::new();
        let mut rx = register(&state, 1);
        handle_message(&state, 1, ip(1), SignalMsg::Create);
        match rx.try_recv().unwrap() {
            SignalMsg::Created { room } => assert_eq!(room.chars().count(), 6),
            other => panic!("expected Created, got {other:?}"),
        }
    }

    #[test]
    fn join_notifies_initiator_then_relays_sdp_and_ice() {
        let state = AppState::new();
        let mut rx1 = register(&state, 1);
        handle_message(&state, 1, ip(1), SignalMsg::Create);
        let room = match rx1.try_recv().unwrap() {
            SignalMsg::Created { room } => room,
            other => panic!("expected Created, got {other:?}"),
        };

        let mut rx2 = register(&state, 2);
        handle_message(&state, 2, ip(2), SignalMsg::Join { room });
        // The initiator (peer 1) is told to start the offer.
        assert!(matches!(rx1.try_recv().unwrap(), SignalMsg::PeerJoined));

        // SDP and ICE relay verbatim to the partner.
        handle_message(&state, 1, ip(1), SignalMsg::Sdp { sdp: "x".into(), kind: "offer".into() });
        assert!(matches!(rx2.try_recv().unwrap(), SignalMsg::Sdp { .. }));
        handle_message(&state, 2, ip(2), SignalMsg::Ice { candidate: "c".into() });
        assert!(matches!(rx1.try_recv().unwrap(), SignalMsg::Ice { .. }));
    }

    #[test]
    fn join_unknown_room_reports_not_found() {
        let state = AppState::new();
        let mut rx = register(&state, 1);
        handle_message(&state, 1, ip(1), SignalMsg::Join { room: "zzzzzz".into() });
        assert!(matches!(rx.try_recv().unwrap(), SignalMsg::RoomNotFound));
    }

    #[test]
    fn third_peer_gets_room_full() {
        let state = AppState::new();
        let mut rx1 = register(&state, 1);
        handle_message(&state, 1, ip(1), SignalMsg::Create);
        let room = match rx1.try_recv().unwrap() {
            SignalMsg::Created { room } => room,
            other => panic!("expected Created, got {other:?}"),
        };
        register(&state, 2);
        handle_message(&state, 2, ip(2), SignalMsg::Join { room: room.clone() });

        let mut rx3 = register(&state, 3);
        handle_message(&state, 3, ip(3), SignalMsg::Join { room });
        assert!(matches!(rx3.try_recv().unwrap(), SignalMsg::RoomFull));
    }

    #[test]
    fn repeated_failed_joins_from_one_ip_get_rate_limited() {
        let state = AppState::new();
        let mut rx = register(&state, 1);
        // Exhaust the failure budget, then one more trips the limiter branch.
        for _ in 0..=JOIN_MAX_FAILURES {
            handle_message(&state, 1, ip(7), SignalMsg::Join { room: "nope12".into() });
            assert!(matches!(rx.try_recv().unwrap(), SignalMsg::RoomNotFound));
        }
    }

    #[test]
    fn server_originated_messages_are_ignored() {
        let state = AppState::new();
        let mut rx = register(&state, 1);
        handle_message(&state, 1, ip(1), SignalMsg::PeerLeft);
        assert!(rx.try_recv().is_err());
    }
}
