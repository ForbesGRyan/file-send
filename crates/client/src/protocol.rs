//! Pure frame encode/decode for the data-channel file protocol.
//!
//! Transfer is one file at a time per direction:
//!   FileStart (JSON string) -> N binary chunks -> FileEnd (JSON string).

use serde::{Deserialize, Serialize};

/// Recommended chunk size in bytes (safe for WebRTC data channels).
pub const CHUNK_SIZE: usize = 16 * 1024;

/// JSON control frame announcing the next file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileStart {
    pub id: u64,
    pub name: String,
    pub size: f64, // f64: files can exceed u32; JS Number-friendly
    pub mime: String,
}

/// JSON control frame signaling a file is fully sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEnd {
    pub id: u64,
}

/// A decoded control frame (the `type`-tagged JSON envelope).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    /// Sender announces a file; no bytes follow until the receiver accepts.
    Offer(FileStart),
    /// Receiver accepts file `id`; the sender then streams it.
    Accept { id: u64 },
    /// Receiver declines file `id`.
    Reject { id: u64 },
    /// Byte stream for a file begins (sent only after an accept).
    Start(FileStart),
    /// Byte stream for a file is complete.
    End(FileEnd),
    /// Receiver aborts an in-progress file `id`; the sender stops streaming it.
    Cancel { id: u64 },
}

/// Encode a control frame to a JSON string (sent as a text data-channel message).
pub fn encode_control(c: &Control) -> String {
    serde_json::to_string(c).expect("control serializes")
}

/// Decode a text data-channel message into a control frame.
pub fn decode_control(s: &str) -> Option<Control> {
    serde_json::from_str(s).ok()
}

/// Split a byte buffer into chunks of at most `CHUNK_SIZE`.
pub fn chunk_bytes(data: &[u8]) -> Vec<&[u8]> {
    if data.is_empty() {
        return vec![];
    }
    data.chunks(CHUNK_SIZE).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_roundtrip_start() {
        let c = Control::Start(FileStart {
            id: 7,
            name: "a.txt".into(),
            size: 1234.0,
            mime: "text/plain".into(),
        });
        let s = encode_control(&c);
        assert_eq!(decode_control(&s), Some(c));
    }

    #[test]
    fn control_roundtrip_end() {
        let c = Control::End(FileEnd { id: 7 });
        let s = encode_control(&c);
        assert_eq!(decode_control(&s), Some(c));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert_eq!(decode_control("not json"), None);
    }

    #[test]
    fn chunks_cover_all_bytes_and_reassemble() {
        let data: Vec<u8> = (0..(CHUNK_SIZE * 2 + 5)).map(|i| (i % 256) as u8).collect();
        let chunks = chunk_bytes(&data);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].len() == CHUNK_SIZE && chunks[1].len() == CHUNK_SIZE);
        assert_eq!(chunks[2].len(), 5);
        let reassembled: Vec<u8> = chunks.concat();
        assert_eq!(reassembled, data);
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_bytes(&[]).is_empty());
    }

    #[test]
    fn control_roundtrip_offer() {
        let c = Control::Offer(FileStart {
            id: 3,
            name: "b.bin".into(),
            size: 99.0,
            mime: "application/octet-stream".into(),
        });
        let s = encode_control(&c);
        assert_eq!(decode_control(&s), Some(c));
    }

    #[test]
    fn control_roundtrip_accept_reject() {
        let a = Control::Accept { id: 5 };
        let r = Control::Reject { id: 6 };
        assert_eq!(decode_control(&encode_control(&a)), Some(a));
        assert_eq!(decode_control(&encode_control(&r)), Some(r));
    }

    #[test]
    fn control_roundtrip_cancel() {
        let c = Control::Cancel { id: 8 };
        assert_eq!(decode_control(&encode_control(&c)), Some(c));
    }

    #[test]
    fn offer_is_type_tagged() {
        let c = Control::Offer(FileStart { id: 1, name: "x".into(), size: 0.0, mime: "".into() });
        assert!(encode_control(&c).contains("\"type\":\"offer\""));
    }
}
