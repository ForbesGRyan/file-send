//! WebSocket signaling: one connection per peer, relays messages to partner.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use shared::SignalMsg;
use tokio::sync::mpsc;

use crate::rooms::{JoinOutcome, PeerId, RoomRegistry};

/// Shared application state, cloned cheaply (Arc) into every handler.
#[derive(Clone)]
pub struct AppState {
    registry: Arc<Mutex<RoomRegistry>>,
    /// Per-peer outbound channel senders, so a peer can push to its partner.
    senders: Arc<Mutex<std::collections::HashMap<PeerId, mpsc::UnboundedSender<SignalMsg>>>>,
    next_id: Arc<AtomicU64>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(RoomRegistry::new())),
            senders: Arc::new(Mutex::new(std::collections::HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
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
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
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
                handle_message(&recv_state, peer_id, parsed);
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

/// Apply one inbound signaling message.
fn handle_message(state: &AppState, peer: PeerId, msg: SignalMsg) {
    match msg {
        SignalMsg::Create => {
            let room_id = gen_room_id();
            let outcome = state.registry.lock().unwrap().create(peer, room_id);
            if let JoinOutcome::Created(room) = outcome {
                state.send_to(peer, SignalMsg::Created { room });
            }
        }
        SignalMsg::Join { room } => {
            let outcome = state.registry.lock().unwrap().join(peer, &room);
            match outcome {
                JoinOutcome::Joined => {
                    // Tell the initiator (the partner) to start the WebRTC offer.
                    if let Some(partner) = state.registry.lock().unwrap().partner(peer) {
                        state.send_to(partner, SignalMsg::PeerJoined);
                    }
                }
                JoinOutcome::Full => state.send_to(peer, SignalMsg::RoomFull),
                JoinOutcome::NotFound => state.send_to(peer, SignalMsg::RoomNotFound),
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
    use super::{gen_room_id, ROOM_ALPHABET};

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
}
