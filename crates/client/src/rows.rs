//! Pure transfer-row state machine.
//!
//! The UI keeps a `Vec<Row>` of transfer rows keyed by `(id, incoming)`. Every
//! transition lives here as a plain function over `&mut Vec<Row>` so the logic
//! is testable without a browser/Leptos runtime; the `App` component is a thin
//! wrapper that owns the signal and calls these.

use crate::filetype::file_kind;
use crate::protocol::FileStart;
use crate::ui::{Transfer as Row, TransferState};

/// Completed fraction (0.0..=1.0). A zero-byte file counts as complete.
pub fn frac(done_bytes: f64, total: f64) -> f64 {
    if total > 0.0 { done_bytes / total } else { 1.0 }
}

/// Find a row by `(id, incoming)` and apply `apply`; if absent, build one with
/// `make`, apply to it, and append. Mirrors the find-or-insert used by the UI.
fn upsert(
    list: &mut Vec<Row>,
    id: u64,
    incoming: bool,
    make: impl FnOnce() -> Row,
    apply: impl FnOnce(&mut Row),
) {
    if let Some(row) = list.iter_mut().find(|r| r.id == id && r.incoming == incoming) {
        apply(row);
    } else {
        let mut row = make();
        apply(&mut row);
        list.push(row);
    }
}

/// An incoming file was offered: insert an `Offered` incoming row if new.
pub fn incoming_offer(list: &mut Vec<Row>, meta: &FileStart) {
    upsert(
        list,
        meta.id,
        true,
        || Row {
            id: meta.id,
            name: meta.name.clone(),
            size: meta.size,
            kind: file_kind(&meta.name, &meta.mime),
            incoming: true,
            fraction: 0.0,
            speed: 0.0,
            state: TransferState::Offered,
        },
        |_r| {},
    );
}

/// Receive progress for an incoming file: mark Active, update fraction + speed.
pub fn recv_progress(list: &mut Vec<Row>, id: u64, name: &str, recv: f64, total: f64, speed: f64) {
    let f = frac(recv, total);
    upsert(
        list,
        id,
        true,
        || Row {
            id,
            name: name.to_string(),
            size: total,
            kind: file_kind(name, ""),
            incoming: true,
            fraction: f,
            speed,
            state: TransferState::Active,
        },
        |r| {
            r.fraction = f;
            r.speed = speed;
            r.state = TransferState::Active;
        },
    );
}

/// An incoming file finished and was saved: mark Done at 100%.
pub fn recv_complete(list: &mut Vec<Row>, id: u64, name: &str) {
    upsert(
        list,
        id,
        true,
        || Row {
            id,
            name: name.to_string(),
            size: 0.0,
            kind: file_kind(name, ""),
            incoming: true,
            fraction: 1.0,
            speed: 0.0,
            state: TransferState::Done,
        },
        |r| {
            r.fraction = 1.0;
            r.speed = 0.0;
            r.state = TransferState::Done;
        },
    );
}

/// Send progress for an outgoing file: Active until 100%, then Done.
pub fn send_progress(list: &mut Vec<Row>, id: u64, name: &str, sent: f64, total: f64) {
    let f = frac(sent, total);
    let done = f >= 1.0;
    let state = if done { TransferState::Done } else { TransferState::Active };
    upsert(
        list,
        id,
        false,
        || Row {
            id,
            name: name.to_string(),
            size: total,
            kind: file_kind(name, ""),
            incoming: false,
            fraction: f,
            speed: 0.0,
            state: state.clone(),
        },
        |r| {
            r.fraction = f;
            r.state = state.clone();
        },
    );
}

/// Append outgoing `Offered` rows for files we've just announced.
pub fn push_outgoing_offer(list: &mut Vec<Row>, id: u64, name: &str, size: f64) {
    list.push(Row {
        id,
        name: name.to_string(),
        size,
        kind: file_kind(name, ""),
        incoming: false,
        fraction: 0.0,
        speed: 0.0,
        state: TransferState::Offered,
    });
}

/// Peer declined our outgoing file: mark it Declined.
pub fn mark_rejected(list: &mut [Row], id: u64) {
    if let Some(r) = list.iter_mut().find(|r| r.id == id && !r.incoming) {
        r.state = TransferState::Declined;
    }
}

/// Peer cancelled our outgoing file mid-send. Don't clobber a row that already
/// finished (a cancel can race the final 100%).
pub fn mark_cancelled_remote(list: &mut [Row], id: u64) {
    if let Some(r) = list.iter_mut().find(|r| r.id == id && !r.incoming) {
        if r.state != TransferState::Done {
            r.state = TransferState::Cancelled;
        }
    }
}

/// Locally accept an incoming offer: leave Offered so the buttons hide.
pub fn accept(list: &mut [Row], id: u64) {
    if let Some(r) = list.iter_mut().find(|r| r.id == id && r.incoming) {
        r.state = TransferState::Active;
    }
}

/// Locally decline an incoming offer: drop the row.
pub fn decline(list: &mut Vec<Row>, id: u64) {
    list.retain(|r| !(r.id == id && r.incoming));
}

/// Locally cancel an in-progress incoming download: mark the row Cancelled.
pub fn cancel(list: &mut [Row], id: u64) {
    if let Some(r) = list.iter_mut().find(|r| r.id == id && r.incoming) {
        r.state = TransferState::Cancelled;
    }
}

/// Ids of incoming rows still awaiting an accept/decline decision.
pub fn pending_incoming_ids(list: &[Row]) -> Vec<u64> {
    list.iter()
        .filter(|r| r.incoming && r.state == TransferState::Offered)
        .map(|r| r.id)
        .collect()
}

/// Accept every incoming row whose id is in `ids` (the "Accept all" action).
pub fn accept_all(list: &mut [Row], ids: &[u64]) {
    for r in list.iter_mut() {
        if r.incoming && ids.contains(&r.id) {
            r.state = TransferState::Active;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: u64, name: &str, size: f64) -> FileStart {
        FileStart { id, name: name.into(), size, mime: String::new() }
    }

    #[test]
    fn frac_handles_zero_byte_files() {
        assert_eq!(frac(0.0, 0.0), 1.0);
        assert_eq!(frac(50.0, 100.0), 0.5);
        assert_eq!(frac(100.0, 100.0), 1.0);
    }

    #[test]
    fn incoming_offer_inserts_once_then_is_idempotent() {
        let mut list = Vec::new();
        incoming_offer(&mut list, &meta(1, "a.pdf", 10.0));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, TransferState::Offered);
        assert!(list[0].incoming);
        assert_eq!(list[0].kind, "PDF");
        // A duplicate offer for the same id must not add a second row.
        incoming_offer(&mut list, &meta(1, "a.pdf", 10.0));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn recv_progress_inserts_then_updates() {
        let mut list = Vec::new();
        recv_progress(&mut list, 1, "a.bin", 25.0, 100.0, 500.0);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].fraction, 0.25);
        assert_eq!(list[0].speed, 500.0);
        assert_eq!(list[0].state, TransferState::Active);
        recv_progress(&mut list, 1, "a.bin", 75.0, 100.0, 800.0);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].fraction, 0.75);
        assert_eq!(list[0].speed, 800.0);
    }

    #[test]
    fn recv_complete_marks_done() {
        let mut list = Vec::new();
        recv_progress(&mut list, 1, "a.bin", 50.0, 100.0, 10.0);
        recv_complete(&mut list, 1, "a.bin");
        assert_eq!(list[0].state, TransferState::Done);
        assert_eq!(list[0].fraction, 1.0);
        assert_eq!(list[0].speed, 0.0);
    }

    #[test]
    fn recv_complete_inserts_when_no_prior_row() {
        let mut list = Vec::new();
        recv_complete(&mut list, 7, "z.txt");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, TransferState::Done);
        assert!(list[0].incoming);
    }

    #[test]
    fn send_progress_active_then_done() {
        let mut list = Vec::new();
        send_progress(&mut list, 1, "a.bin", 30.0, 100.0);
        assert!(!list[0].incoming);
        assert_eq!(list[0].state, TransferState::Active);
        assert_eq!(list[0].fraction, 0.3);
        send_progress(&mut list, 1, "a.bin", 100.0, 100.0);
        assert_eq!(list[0].state, TransferState::Done);
        assert_eq!(list[0].fraction, 1.0);
    }

    #[test]
    fn send_progress_zero_byte_file_is_done_immediately() {
        let mut list = Vec::new();
        send_progress(&mut list, 1, "empty", 0.0, 0.0);
        assert_eq!(list[0].state, TransferState::Done);
        assert_eq!(list[0].fraction, 1.0);
    }

    #[test]
    fn incoming_and_outgoing_rows_with_same_id_are_distinct() {
        let mut list = Vec::new();
        recv_progress(&mut list, 1, "in", 10.0, 100.0, 0.0);
        send_progress(&mut list, 1, "out", 10.0, 100.0);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn push_outgoing_offer_appends_offered_row() {
        let mut list = Vec::new();
        push_outgoing_offer(&mut list, 3, "doc.zip", 99.0);
        assert_eq!(list[0].state, TransferState::Offered);
        assert!(!list[0].incoming);
        assert_eq!(list[0].size, 99.0);
    }

    #[test]
    fn mark_rejected_only_hits_outgoing() {
        let mut list = Vec::new();
        push_outgoing_offer(&mut list, 1, "x", 1.0);
        incoming_offer(&mut list, &meta(1, "x", 1.0));
        mark_rejected(&mut list, 1);
        let out = list.iter().find(|r| !r.incoming).unwrap();
        let inc = list.iter().find(|r| r.incoming).unwrap();
        assert_eq!(out.state, TransferState::Declined);
        assert_eq!(inc.state, TransferState::Offered);
    }

    #[test]
    fn mark_cancelled_remote_skips_done_rows() {
        let mut list = Vec::new();
        send_progress(&mut list, 1, "x", 100.0, 100.0); // Done
        mark_cancelled_remote(&mut list, 1);
        assert_eq!(list[0].state, TransferState::Done);
        send_progress(&mut list, 2, "y", 10.0, 100.0); // Active
        mark_cancelled_remote(&mut list, 2);
        assert_eq!(list[1].state, TransferState::Cancelled);
    }

    #[test]
    fn accept_decline_cancel_local_actions() {
        let mut list = Vec::new();
        incoming_offer(&mut list, &meta(1, "a", 1.0));
        accept(&mut list, 1);
        assert_eq!(list[0].state, TransferState::Active);
        cancel(&mut list, 1);
        assert_eq!(list[0].state, TransferState::Cancelled);

        incoming_offer(&mut list, &meta(2, "b", 1.0));
        decline(&mut list, 2);
        assert!(list.iter().all(|r| r.id != 2));
    }

    #[test]
    fn accept_all_targets_only_pending_incoming() {
        let mut list = Vec::new();
        incoming_offer(&mut list, &meta(1, "a", 1.0));
        incoming_offer(&mut list, &meta(2, "b", 1.0));
        push_outgoing_offer(&mut list, 3, "c", 1.0); // outgoing, must be ignored
        let ids = pending_incoming_ids(&list);
        assert_eq!(ids, vec![1, 2]);
        accept_all(&mut list, &ids);
        assert_eq!(list[0].state, TransferState::Active);
        assert_eq!(list[1].state, TransferState::Active);
        assert_eq!(list[2].state, TransferState::Offered); // outgoing untouched
    }
}
