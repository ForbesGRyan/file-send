use serde::{Deserialize, Serialize};

/// Messages exchanged over the signaling WebSocket between a client and the
/// server. Serialized as JSON with an internal `type` tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMsg {
    /// Client → server: create a fresh room. Server replies with `Created`.
    Create,
    /// Server → client: a room was created with this id (creator is initiator).
    Created { room: String },
    /// Client → server: join an existing room by id.
    Join { room: String },
    /// Server → client: the second peer joined; initiator should start the offer.
    PeerJoined,
    /// Server → client: the other peer left or disconnected.
    PeerLeft,
    /// Server → client: the room is already full (two peers).
    RoomFull,
    /// Server → client: no room exists with the requested id.
    RoomNotFound,
    /// Relayed both ways: SDP offer/answer payload.
    Sdp { sdp: String, kind: String },
    /// Relayed both ways: a single ICE candidate (JSON-encoded init object).
    Ice { candidate: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: &SignalMsg) {
        let json = serde_json::to_string(msg).unwrap();
        let back: SignalMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(*msg, back);
    }

    #[test]
    fn roundtrips_all_variants() {
        roundtrip(&SignalMsg::Create);
        roundtrip(&SignalMsg::Created { room: "abc".into() });
        roundtrip(&SignalMsg::Join { room: "abc".into() });
        roundtrip(&SignalMsg::PeerJoined);
        roundtrip(&SignalMsg::PeerLeft);
        roundtrip(&SignalMsg::RoomFull);
        roundtrip(&SignalMsg::RoomNotFound);
        roundtrip(&SignalMsg::Sdp { sdp: "v=0".into(), kind: "offer".into() });
        roundtrip(&SignalMsg::Ice { candidate: "{}".into() });
    }

    #[test]
    fn uses_type_tag() {
        let json = serde_json::to_string(&SignalMsg::Join { room: "x".into() }).unwrap();
        assert_eq!(json, r#"{"type":"join","room":"x"}"#);
    }
}
