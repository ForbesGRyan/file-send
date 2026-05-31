# Accept-Before-Transfer (AirDrop-style) Design

Date: 2026-05-31
Status: Approved (design), pending implementation plan

## Goal

Let the receiving peer accept or decline each incoming file **before any bytes
are transferred**, instead of today's behavior where bytes stream automatically
and the browser saves the file on completion. Several files offered at once can
be accepted/declined individually, with an "Accept all" convenience action.

## Background (current behavior)

- `send_files` (in `crates/client/src/transfer.rs`) loops over the chosen files
  and, for each, calls `send_file`, which sends `Control::Start(FileStart)` then
  binary chunks then `Control::End(FileEnd)`.
- `attach_receiver` sets the data channel's `onmessage`; on `Start` it records
  metadata, on chunks it accumulates + reports progress, on `End` it assembles a
  Blob and **auto-triggers a browser download**.
- The data channel is bidirectional, so receiver→sender control frames are
  possible; today only sender→receiver frames are used.
- File ids are `i as u64` per batch (restart at 0 each drop) — fine today, but
  ambiguous for an accept handshake; see "File ids" below.
- A pre-connect queue (already implemented) holds files chosen before the
  channel is open and flushes them on `onopen`.

## Protocol (`crates/client/src/protocol.rs`)

Extend the `Control` enum with three new variants; `Start`/`End` keep their
exact current meaning (the byte phase), now sent only after an accept:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Offer(FileStart),   // NEW: sender announces a file (id, name, size, mime); no bytes
    Accept { id: u64 }, // NEW: receiver accepts file `id`
    Reject { id: u64 }, // NEW: receiver declines file `id`
    Start(FileStart),   // existing: byte stream for `id` begins
    End(FileEnd),       // existing: byte stream for `id` done
}
```

`FileStart`/`FileEnd` are unchanged. `encode_control`/`decode_control` are
unchanged (serde handles the new variants).

## Transfer flow

1. **Offer:** when files are chosen (and the channel is open — otherwise the
   existing pre-connect queue holds them and flushes on open), the sender sends
   one `Offer(FileStart)` per file, each with a session-unique `id`, and records
   `id -> File` in sender state. Bytes are NOT sent yet.
2. **Decide:** the receiver shows one pending row per offer with **Accept** and
   **Decline** buttons; when ≥2 offers are pending, an **Accept all** button
   accepts every pending offer.
3. **Accept:** the receiver sends `Accept { id }`. The sender enqueues `id` and a
   single streaming task drains the queue **sequentially**, streaming each
   accepted file via the existing `send_file` (`Start`→chunks→`End`) with
   backpressure. One active incoming transfer at a time is preserved.
4. **Receive:** the receiver handles `Start`/chunks/`End` exactly as today and
   saves the file on `End` (consent already given at step 3 — no second prompt).
5. **Decline:** the receiver sends `Reject { id }`. The sender drops that file
   from its state and marks it declined; remaining files are unaffected.

## File ids

Replace the per-batch `i as u64` with a **session-unique monotonic counter** on
the sender (a `Cell<u64>` / `Rc<Cell<u64>>` in the client, incremented per
offered file). This guarantees `Accept{id}`/`Reject{id}` map to exactly one
offered file even across multiple drops, and resolves the previously-deferred
per-batch id-collision issue.

## `transfer.rs` structure

The data channel has a single `onmessage`; it becomes a **unified control
router** that decodes each control frame and dispatches:

- `Offer(meta)` → invoke an `on_offer(meta)` callback (UI adds a pending
  incoming row). Receiver role.
- `Accept { id }` → mark the sender's offered file `id` accepted: enqueue it for
  streaming. Sender role.
- `Reject { id }` → drop offered file `id`; invoke an `on_rejected(id)` callback
  (UI marks the outgoing row declined). Sender role.
- `Start(meta)` → begin receiving (set active incoming = meta). Receiver role.
- binary chunk → accumulate + `on_progress`. Receiver role.
- `End { id }` → finalize (assemble Blob, trigger download) + `on_complete`.
  Receiver role.

Sender-side state (per peer connection):

- `offered: HashMap<u64, File>` — files announced, awaiting a decision.
- An accepted-id queue plus a single async drain task that streams accepted
  files one at a time using `send_file`. Reuses the existing chunking +
  `BUFFER_HIGH` backpressure.

Public functions (replacing/expanding the current `send_files`):

- `offer_files(dc, next_id, files, on_offer_local)` — assign ids, send `Offer`s,
  record sender state, and surface the outgoing rows locally (so the sender sees
  "waiting for peer…").
- `attach_channel(dc, callbacks...)` — the unified router wiring receive +
  accept/reject handling (supersedes `attach_receiver`).
- A receiver-initiated `accept(dc, id)` / `reject(dc, id)` that send the
  corresponding control frame.

Exact signatures are finalized in the implementation plan; this section fixes
responsibilities and boundaries.

## UI (`ui.rs` / `app.rs`)

A transfer row gains an explicit state:

- **Offered** — incoming: shows file name + size + **Accept** / **Decline**
  buttons; outgoing: shows "waiting for peer…".
- **Active** — transferring; shows the `%` bar (as today).
- **Done** — `✓ DONE` (as today).
- **Declined** — `✗ DECLINED`.

`FileProgress` (or a renamed transfer-item struct) gains a `state` enum and, for
incoming offers, the `id` and `size` needed to render the prompt and to send
`Accept{id}`/`Reject{id}`. An **Accept all** button appears when ≥2 incoming
offers are pending and accepts each.

`app.rs` wiring:

- `on_offer(meta)` → push an Offered (incoming) row.
- Accept button → `transfer::accept(dc, id)`; the row flips to Active when bytes
  start. Decline → `transfer::reject(dc, id)`; row → Declined.
- Outgoing: `offer_files` adds Offered (outgoing) rows; `on_rejected(id)` →
  Declined; acceptance → Active via the existing upload progress callback.
- The pre-connect queue now stores files and sends their `Offer`s on `onopen`
  (the queue/flush mechanism already exists; only the flush action changes from
  "send bytes" to "send offers").

## Error handling

- Peer disconnects mid-offer or mid-transfer → existing `PeerLeft` status; no
  special handling beyond what exists.
- `Reject` → outgoing row shows `✗ DECLINED`; the batch continues.
- Unknown/garbage/unexpected control frame (e.g. `Accept` for an unknown id) →
  ignored (the router's catch-all), never panics.
- A 0-byte accepted file streams `Start`→`End` with no chunks and finalizes
  correctly (as today).

## Testing

- **Unit (`protocol.rs`):** roundtrip encode/decode for `Offer`, `Accept`,
  `Reject`; `decode_control` still rejects garbage.
- **Unit (pure helpers):** any extracted pure logic (e.g. id assignment, the
  decision that maps a control frame to a router action if expressed purely).
- **Browser (manual, two tabs):** the project's established verification path.
  Confirm: (a) sender offers, no bytes until accept; (b) Accept → file transfers
  and saves, both rows reach `✓ DONE`; (c) Decline → sender shows `✗ DECLINED`,
  no bytes; (d) multiple files: per-file Accept/Decline and **Accept all**;
  (e) both directions (host→joiner and joiner→host); (f) files staged before the
  peer connects still produce offers once connected.

## Out of scope

- Concurrent/parallel streaming of multiple accepted files (kept sequential).
- Resuming/canceling an in-progress transfer.
- Server changes; TURN; any change to signaling.

## Success criteria

- No bytes for a file are sent until the receiver accepts it.
- Per-file Accept/Decline works; Accept all accepts every pending incoming
  offer.
- Declined files transfer no bytes and show a clear declined state on the
  sender.
- Accepted files transfer and save exactly as today, both directions.
- Files chosen before connect are offered (not lost) once connected.
- `protocol.rs` new-variant roundtrip tests pass; existing tests still pass.
