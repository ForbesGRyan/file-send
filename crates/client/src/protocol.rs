//! Pure frame encode/decode for the data-channel file protocol.
//!
//! Transfer is one file at a time per direction:
//!   FileStart (JSON string) -> N binary chunks -> FileEnd (JSON string).

use serde::{Deserialize, Serialize};

/// Bytes per data-channel message. 256 KB sits within the SCTP
/// `maxMessageSize` that modern browsers negotiate; the sender still clamps to
/// the channel's actual limit at runtime in case a peer advertises less.
pub const CHUNK_SIZE: usize = 256 * 1024;

/// Bytes read from the `File` per async `arrayBuffer()` await. The sender reads
/// in large slabs and slices them in memory, so it pays one event-loop round
/// trip per slab instead of one per [`CHUNK_SIZE`] message. That per-await
/// latency — not the link — is what caps throughput, so a big slab matters more
/// than a big chunk.
pub const SLAB_SIZE: usize = 8 * 1024 * 1024;

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

/// Split `[0, total)` into consecutive half-open `(start, end)` byte ranges of
/// at most `step` bytes (the last range holds the remainder). A `step` of 0,
/// or a `total` of 0, yields no ranges. This drives both the slab reads and the
/// per-message chunking on the send path, and is the unit under the throughput
/// test: fewer slab ranges == fewer awaited file reads == higher throughput.
pub fn split_ranges(total: u64, step: usize) -> Vec<(u64, u64)> {
    if step == 0 {
        return vec![];
    }
    let step = step as u64;
    let mut ranges = Vec::new();
    let mut start = 0u64;
    while start < total {
        let end = (start + step).min(total);
        ranges.push((start, end));
        start = end;
    }
    ranges
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
    fn split_ranges_are_contiguous_and_cover_total() {
        let total = SLAB_SIZE as u64 * 2 + 123;
        let ranges = split_ranges(total, SLAB_SIZE);
        assert_eq!(ranges.first().unwrap().0, 0, "starts at 0");
        assert_eq!(ranges.last().unwrap().1, total, "ends at total");
        for w in ranges.windows(2) {
            assert_eq!(w[0].1, w[1].0, "no gaps or overlap between ranges");
        }
        let covered: u64 = ranges.iter().map(|(s, e)| e - s).sum();
        assert_eq!(covered, total, "every byte covered exactly once");
        // All but the last are full-size; the last is the remainder.
        assert!(ranges[..ranges.len() - 1].iter().all(|(s, e)| e - s == SLAB_SIZE as u64));
        assert_eq!(ranges.last().unwrap().1 - ranges.last().unwrap().0, 123);
    }

    #[test]
    fn split_ranges_handles_empty_and_zero_step() {
        assert!(split_ranges(0, SLAB_SIZE).is_empty(), "zero-byte file: no reads");
        assert!(split_ranges(100, 0).is_empty(), "zero step: no ranges (no infinite loop)");
    }

    /// Throughput check. We established the transfer is latency-bound: the cap is
    /// the number of awaited `File` reads (one event-loop round trip each), not
    /// the link. Reading in [`SLAB_SIZE`] slabs instead of one await per
    /// 16-KB chunk (the old behavior) must cut those reads by orders of
    /// magnitude — that reduction *is* the speedup.
    #[test]
    fn slab_reads_collapse_event_loop_round_trips() {
        let size = 256 * 1024 * 1024; // a 256 MB transfer
        let old_per_chunk_reads = split_ranges(size, 16 * 1024).len(); // pre-change loop
        let new_slab_reads = split_ranges(size, SLAB_SIZE).len();
        assert_eq!(old_per_chunk_reads, 16_384, "old: one awaited read per 16 KB");
        assert_eq!(new_slab_reads, 32, "new: one awaited read per 8 MB slab");
        assert!(
            old_per_chunk_reads / new_slab_reads >= 100,
            "expected >=100x fewer awaited reads, got {}x",
            old_per_chunk_reads / new_slab_reads
        );
    }

    #[test]
    fn offer_is_type_tagged() {
        let c = Control::Offer(FileStart { id: 1, name: "x".into(), size: 0.0, mime: "".into() });
        assert!(encode_control(&c).contains("\"type\":\"offer\""));
    }
}
