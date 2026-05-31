# Accept-Before-Transfer (AirDrop-style) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the receiver accept or decline each incoming file before any bytes transfer (per-file plus an "Accept all"), instead of auto-receiving and auto-saving.

**Architecture:** Extend the data-channel `Control` protocol with `Offer`/`Accept`/`Reject`. The sender announces files with `Offer` (no bytes); the receiver replies `Accept{id}`/`Reject{id}`; accepted files stream sequentially via the existing `Start`→chunks→`End`. `transfer.rs` becomes a `Transfer` handle wrapping the channel with a unified control router (handles both send and receive roles) plus a sequential sender drain task. The UI gains per-file transfer states (Offered/Active/Done/Declined) with Accept/Decline buttons.

**Tech Stack:** Rust (edition 2024), Leptos 0.8 (CSR/WASM), Trunk, web-sys RtcDataChannel, serde.

---

## File Structure

- **Modify** `crates/client/src/protocol.rs` — add `Offer`/`Accept`/`Reject` to `Control`; roundtrip tests.
- **Modify** `crates/client/src/transfer.rs` — full rewrite: `Transfer` handle + `Handlers` + unified router + sequential sender drain. Keeps `send_file`, `yield_to_event_loop`, `trigger_download` verbatim. Removes `attach_receiver` and `send_files`.
- **Modify** `crates/client/src/ui.rs` — replace `FileProgress`/`ProgressList` with `Transfer`/`TransferState` + a state-aware `ProgressList` (Accept/Decline/Accept-all). Add pure `fmt_size`. Keep `Status`, `StatusBar`, `ShareLink`, `JoinBox`.
- **Modify** `crates/client/src/app.rs` — build `Handlers`, store a `Transfer` instead of a raw channel, offer files (queue before open), wire Accept/Decline/Accept-all, render new `ProgressList`.

Build/verify commands:
- Unit tests (host): `cargo test -p client`
- WASM build: `cd crates/client && trunk build`

**Compile-unit note:** Task 1 (protocol) stands alone. Tasks 2, 3, 4 form ONE compile unit — `ui.rs` removes `FileProgress`, `transfer.rs` removes `send_files`/`attach_receiver`, and `app.rs` is rewritten to match. Make all three edits, then build once, then commit them together (the Task 4 commit covers Tasks 2–4).

---

## Task 1: Protocol — Offer/Accept/Reject

**Files:**
- Modify: `crates/client/src/protocol.rs`

- [ ] **Step 1: Write failing roundtrip tests**

In `crates/client/src/protocol.rs`, inside `mod tests` (after `control_roundtrip_end`), add:

```rust
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
    fn offer_is_type_tagged() {
        let c = Control::Offer(FileStart { id: 1, name: "x".into(), size: 0.0, mime: "".into() });
        assert!(encode_control(&c).contains("\"type\":\"offer\""));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p client protocol`
Expected: FAIL — no variants `Offer`, `Accept`, `Reject` on `Control`.

- [ ] **Step 3: Add the variants**

In `crates/client/src/protocol.rs`, replace the `Control` enum with:

```rust
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
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p client protocol`
Expected: PASS (all protocol tests, including the 3 new ones).

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/protocol.rs
git commit -m "feat(protocol): add Offer/Accept/Reject control frames"
```

End the commit body with:
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

## Task 2: UI model + components (`ui.rs`)

**Files:**
- Modify: `crates/client/src/ui.rs`

> Part of the Tasks 2–4 compile unit. TDD applies to `fmt_size` (host-testable); the components are compile-verified in Task 4.

- [ ] **Step 1: Write the failing test for `fmt_size`**

At the bottom of `crates/client/src/ui.rs`, add a test module:

```rust
#[cfg(test)]
mod tests {
    use super::fmt_size;

    #[test]
    fn formats_human_sizes() {
        assert_eq!(fmt_size(0.0), "0 B");
        assert_eq!(fmt_size(512.0), "512 B");
        assert_eq!(fmt_size(1024.0), "1.0 KB");
        assert_eq!(fmt_size(1536.0), "1.5 KB");
        assert_eq!(fmt_size(1_048_576.0), "1.0 MB");
        assert_eq!(fmt_size(1_073_741_824.0), "1.0 GB");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p client fmt_size`
Expected: FAIL — `cannot find function fmt_size`.

- [ ] **Step 3: Implement `fmt_size` and the transfer model**

In `crates/client/src/ui.rs`, replace the `FileProgress` struct AND the `ProgressList` component with the following (leave `Status`, `StatusBar`, `ShareLink`, `JoinBox` unchanged):

```rust
/// Format a byte count as a short human string (e.g. "1.5 KB").
pub fn fmt_size(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", v as u64, UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Lifecycle state of one transfer row.
#[derive(Clone, PartialEq)]
pub enum TransferState {
    /// Incoming: awaiting the local user's accept/decline. Outgoing: awaiting the peer.
    Offered,
    /// Bytes are flowing.
    Active,
    /// Finished and saved (incoming) or fully sent (outgoing).
    Done,
    /// Declined by the deciding side.
    Declined,
}

/// One transfer's UI row. Rows are keyed by `(id, incoming)`.
#[derive(Clone, PartialEq)]
pub struct Transfer {
    pub id: u64,
    pub name: String,
    pub size: f64,
    pub kind: &'static str, // type badge, e.g. "PDF"
    pub incoming: bool,     // true = receiving (peer -> me); false = sending
    pub fraction: f64,      // 0.0..=1.0
    pub state: TransferState,
}

#[component]
pub fn ProgressList(
    items: ReadSignal<Vec<Transfer>>,
    on_accept: Callback<u64>,
    on_decline: Callback<u64>,
    on_accept_all: Callback<()>,
) -> impl IntoView {
    // Show "Accept all" only when 2+ incoming offers are pending.
    let show_accept_all = move || {
        items.get().iter().filter(|t| t.incoming && t.state == TransferState::Offered).count() >= 2
    };
    view! {
        <Show when=show_accept_all>
            <button class="acceptall" on:click=move |_| on_accept_all.run(())>
                "Accept all"
            </button>
        </Show>
        <ul class="progress-list">
            {move || {
                items
                    .get()
                    .into_iter()
                    .map(|t| transfer_row(t, on_accept, on_decline))
                    .collect_view()
            }}
        </ul>
    }
}

/// Render one transfer row according to its state.
fn transfer_row(t: Transfer, on_accept: Callback<u64>, on_decline: Callback<u64>) -> impl IntoView {
    let id = t.id;
    let pct = (t.fraction * 100.0).round();
    let arrow = if t.incoming { "↓" } else { "↑" };
    match t.state {
        TransferState::Offered if t.incoming => view! {
            <li class="row offer">
                <div class="top">
                    <span>
                        <span class="diricon">{arrow}</span>" "
                        <span class="name">{t.name.clone()}</span>" "
                        <span class="tag">{t.kind}</span>" "
                        <span class="size">{fmt_size(t.size)}</span>
                    </span>
                    <span class="actions">
                        <button class="accept" on:click=move |_| on_accept.run(id)>"Accept"</button>
                        <button class="decline" on:click=move |_| on_decline.run(id)>"Decline"</button>
                    </span>
                </div>
            </li>
        }
        .into_any(),
        TransferState::Offered => view! {
            <li class="row waiting">
                <div class="top">
                    <span>
                        <span class="diricon">{arrow}</span>" "
                        <span class="name">{t.name.clone()}</span>" "
                        <span class="tag">{t.kind}</span>
                    </span>
                    <span class="pct">"WAITING…"</span>
                </div>
            </li>
        }
        .into_any(),
        TransferState::Declined => view! {
            <li class="row declined">
                <div class="top">
                    <span>
                        <span class="diricon">{arrow}</span>" "
                        <span class="name">{t.name.clone()}</span>" "
                        <span class="tag">{t.kind}</span>
                    </span>
                    <span class="pct">"✗ DECLINED"</span>
                </div>
            </li>
        }
        .into_any(),
        TransferState::Active | TransferState::Done => {
            let done = t.state == TransferState::Done;
            let row_class = if done { "row done" } else { "row" };
            let pct_label = if done { "✓ DONE".to_string() } else { format!("{pct}%") };
            let bar_style = format!("width:{pct}%");
            view! {
                <li class=row_class>
                    <div class="top">
                        <span>
                            <span class="diricon">{arrow}</span>" "
                            <span class="name">{t.name.clone()}</span>" "
                            <span class="tag">{t.kind}</span>
                        </span>
                        <span class="pct">{pct_label}</span>
                    </div>
                    <div class="bar"><i style=bar_style></i></div>
                </li>
            }
            .into_any()
        }
    }
}
```

> Note: `transfer_row` returns differing view types per arm, so each arm uses `.into_any()` (Leptos `IntoAny`). `Callback` and `IntoAny` come from `leptos::prelude::*` (already imported at the top of `ui.rs`).

- [ ] **Step 4: Run the `fmt_size` test**

Run: `cargo test -p client fmt_size`
Expected: PASS. (The components are compiled in Task 4; `cargo test` compiles the host target and will succeed for `fmt_size` even though `app.rs` is not yet updated — if the host build fails only due to `app.rs`/`transfer.rs` references, that is expected and resolved in Tasks 3–4. Run this test again at the end of Task 4 to confirm.)

(No commit yet — committed with Tasks 3–4.)

---

## Task 3: Transfer engine rewrite (`transfer.rs`)

**Files:**
- Modify: `crates/client/src/transfer.rs`

> Part of the Tasks 2–4 compile unit. Compile-verified in Task 4.

- [ ] **Step 1: Replace the file contents**

Replace the ENTIRE contents of `crates/client/src/transfer.rs` with:

```rust
//! File transfer over the data channel with an accept-before-transfer handshake.
//!
//! The sender announces each file with `Offer` (no bytes). The receiver replies
//! `Accept{id}` or `Reject{id}`. Accepted files are streamed sequentially as
//! `Start` -> binary chunks -> `End`, reassembled, and saved.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Blob, File, MessageEvent, RtcDataChannel, Url};

use crate::protocol::{decode_control, encode_control, Control, FileEnd, FileStart, CHUNK_SIZE};

/// High-water mark for buffered bytes; pause sending above this.
const BUFFER_HIGH: f64 = 1_000_000.0;

/// UI callbacks for transfer events. All are `Rc<dyn Fn..>` so they can be
/// cloned into the message router and the async sender task.
#[derive(Clone)]
pub struct Handlers {
    /// An incoming file was offered; show an accept/decline prompt.
    pub on_offer: Rc<dyn Fn(FileStart)>,
    /// Receiving progress for incoming file `id`: (id, name, received, total).
    pub on_recv_progress: Rc<dyn Fn(u64, String, f64, f64)>,
    /// Incoming file `id` finished and was saved: (id, name).
    pub on_recv_complete: Rc<dyn Fn(u64, String)>,
    /// Sending progress for outgoing file `id`: (id, name, sent, total).
    pub on_send_progress: Rc<dyn Fn(u64, String, f64, f64)>,
    /// Outgoing file `id` was declined by the peer.
    pub on_rejected: Rc<dyn Fn(u64)>,
}

/// Receiver state for the current incoming file.
#[derive(Default)]
struct Incoming {
    meta: Option<FileStart>,
    chunks: Vec<js_sys::Uint8Array>,
    received: f64,
}

/// Sender state: offered files awaiting a decision, the accepted queue, and a
/// flag indicating the drain task is running.
#[derive(Default)]
struct Outgoing {
    offered: HashMap<u64, File>,
    queue: VecDeque<(u64, File)>,
    sending: bool,
}

/// State shared between the control router and the sender drain task.
struct Shared {
    dc: RtcDataChannel,
    handlers: Handlers,
    next_id: Cell<u64>,
    incoming: RefCell<Incoming>,
    outgoing: RefCell<Outgoing>,
}

/// Handle to a data channel wired for the accept-before-transfer protocol.
#[derive(Clone)]
pub struct Transfer {
    shared: Rc<Shared>,
}

impl Transfer {
    /// Wrap `dc`: install the control router and return a handle.
    pub fn new(dc: RtcDataChannel, handlers: Handlers) -> Transfer {
        dc.set_binary_type(web_sys::RtcDataChannelType::Arraybuffer);
        let shared = Rc::new(Shared {
            dc,
            handlers,
            next_id: Cell::new(0),
            incoming: RefCell::new(Incoming::default()),
            outgoing: RefCell::new(Outgoing::default()),
        });
        install_router(&shared);
        Transfer { shared }
    }

    /// Is the underlying channel open?
    pub fn is_open(&self) -> bool {
        self.shared.dc.ready_state() == web_sys::RtcDataChannelState::Open
    }

    /// Announce `files` to the peer (one `Offer` each). Returns the offered
    /// `(id, name, size)` list so the caller can render outgoing rows.
    pub fn offer_files(&self, files: Vec<File>) -> Vec<(u64, String, f64)> {
        let mut offered = Vec::new();
        for file in files {
            let id = self.shared.next_id.get();
            self.shared.next_id.set(id + 1);
            let meta = FileStart {
                id,
                name: file.name(),
                size: file.size(),
                mime: file.type_(),
            };
            let _ = self.shared.dc.send_with_str(&encode_control(&Control::Offer(meta.clone())));
            offered.push((id, meta.name.clone(), meta.size));
            self.shared.outgoing.borrow_mut().offered.insert(id, file);
        }
        offered
    }

    /// Accept an incoming offered file (sends `Accept{id}` to the sender).
    pub fn accept(&self, id: u64) {
        let _ = self.shared.dc.send_with_str(&encode_control(&Control::Accept { id }));
    }

    /// Decline an incoming offered file (sends `Reject{id}`).
    pub fn reject(&self, id: u64) {
        let _ = self.shared.dc.send_with_str(&encode_control(&Control::Reject { id }));
    }
}

/// Install the unified control/message router on the channel.
fn install_router(shared: &Rc<Shared>) {
    let shared = shared.clone();
    let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        let data = e.data();
        if let Some(text) = data.as_string() {
            match decode_control(&text) {
                // Receiver role.
                Some(Control::Offer(meta)) => (shared.handlers.on_offer)(meta),
                Some(Control::Start(meta)) => {
                    let mut inc = shared.incoming.borrow_mut();
                    inc.meta = Some(meta);
                    inc.chunks.clear();
                    inc.received = 0.0;
                }
                Some(Control::End(_)) => finalize(&shared),
                // Sender role.
                Some(Control::Accept { id }) => on_accept(&shared, id),
                Some(Control::Reject { id }) => {
                    shared.outgoing.borrow_mut().offered.remove(&id);
                    (shared.handlers.on_rejected)(id);
                }
                None => {}
            }
        } else {
            // Binary chunk (receiver role).
            let array = js_sys::Uint8Array::new(&data);
            let progress = {
                let mut inc = shared.incoming.borrow_mut();
                inc.received += array.length() as f64;
                inc.chunks.push(array);
                inc.meta.as_ref().map(|m| (m.id, m.name.clone(), m.size, inc.received))
            };
            if let Some((id, name, total, recv)) = progress {
                (shared.handlers.on_recv_progress)(id, name, recv, total);
            }
        }
    });
    shared.dc.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
}

/// Sender: an offered file was accepted — queue it and start draining if idle.
fn on_accept(shared: &Rc<Shared>, id: u64) {
    let start_drain = {
        let mut out = shared.outgoing.borrow_mut();
        if let Some(file) = out.offered.remove(&id) {
            out.queue.push_back((id, file));
        }
        if !out.sending && !out.queue.is_empty() {
            out.sending = true;
            true
        } else {
            false
        }
    };
    if start_drain {
        spawn_local(drain(shared.clone()));
    }
}

/// Sender: stream accepted files one at a time until the queue is empty.
async fn drain(shared: Rc<Shared>) {
    loop {
        let next = shared.outgoing.borrow_mut().queue.pop_front();
        let Some((id, file)) = next else {
            shared.outgoing.borrow_mut().sending = false;
            return;
        };
        let name = file.name();
        let total = file.size();
        let prog = {
            let shared = shared.clone();
            let name = name.clone();
            move |sent: f64| (shared.handlers.on_send_progress)(id, name.clone(), sent, total)
        };
        let _ = send_file(shared.dc.clone(), id, file, prog).await;
        // Guarantee a final 100% (also covers 0-byte files that emit no chunks).
        (shared.handlers.on_send_progress)(id, name, total, total);
    }
}

/// Send one file: `Start` -> chunks (with backpressure) -> `End`.
async fn send_file(
    dc: RtcDataChannel,
    id: u64,
    file: File,
    on_progress: impl Fn(f64) + 'static,
) -> Result<(), JsValue> {
    let size = file.size();
    let start = Control::Start(FileStart {
        id,
        name: file.name(),
        size,
        mime: file.type_(),
    });
    dc.send_with_str(&encode_control(&start))?;

    let mut offset: f64 = 0.0;
    let mut sent: f64 = 0.0;
    while offset < size {
        let end = (offset + CHUNK_SIZE as f64).min(size);
        let blob = file.slice_with_f64_and_f64(offset, end)?;
        let buf = JsFuture::from(blob.array_buffer()).await?;
        let array = js_sys::Uint8Array::new(&buf);

        while dc.buffered_amount() as f64 > BUFFER_HIGH {
            yield_to_event_loop().await;
        }

        dc.send_with_array_buffer(&array.buffer())?;
        sent += array.length() as f64;
        offset = end;
        on_progress(sent);
    }

    dc.send_with_str(&encode_control(&Control::End(FileEnd { id })))?;
    Ok(())
}

/// Yield control to the browser event loop for one timer tick.
async fn yield_to_event_loop() {
    let _ = JsFuture::from(js_sys::Promise::new(&mut |resolve, _| {
        let window = web_sys::window().unwrap();
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
    }))
    .await;
}

/// Assemble received chunks into a Blob and trigger a browser download.
fn finalize(shared: &Rc<Shared>) {
    let (meta, parts) = {
        let mut inc = shared.incoming.borrow_mut();
        let meta = inc.meta.take();
        let parts = std::mem::take(&mut inc.chunks);
        inc.received = 0.0;
        (meta, parts)
    };
    let Some(meta) = meta else { return };

    let blob_parts = js_sys::Array::new();
    for p in &parts {
        blob_parts.push(p);
    }
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(&meta.mime);
    let Ok(blob) = Blob::new_with_u8_array_sequence_and_options(&blob_parts, &options) else {
        return;
    };
    trigger_download(&blob, &meta.name);
    (shared.handlers.on_recv_complete)(meta.id, meta.name);
}

/// Create an object URL for the blob and click a temporary anchor to download.
fn trigger_download(blob: &Blob, filename: &str) {
    let Ok(url) = Url::create_object_url_with_blob(blob) else { return };
    let document = web_sys::window().unwrap().document().unwrap();
    let anchor = document.create_element("a").unwrap();
    let anchor: web_sys::HtmlAnchorElement = anchor.unchecked_into();
    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.click();
    // Defer revocation by one event-loop tick so it can't race the browser's
    // asynchronous download dispatch (a Safari edge case if revoked inline).
    spawn_local(async move {
        yield_to_event_loop().await;
        let _ = Url::revoke_object_url(&url);
    });
}
```

(No commit yet — committed with Task 4.)

---

## Task 4: Wire it up (`app.rs`) + green build (covers Tasks 2–4)

**Files:**
- Modify: `crates/client/src/app.rs`

- [ ] **Step 1: Update imports**

In `crates/client/src/app.rs`, replace the `use crate::...` and the `web_sys` import lines with:

```rust
use web_sys::{DragEvent, HtmlInputElement, RtcPeerConnection, RtcSdpType};

use crate::filetype::file_kind;
use crate::protocol::FileStart;
use crate::qr::qr_svg;
use crate::signaling::Signaling;
use crate::transfer::{Handlers, Transfer};
use crate::ui::{JoinBox, ProgressList, ShareLink, Status, StatusBar, Transfer as Row, TransferState};
```

(`RtcDataChannel`/`RtcDataChannelState` are no longer referenced directly in `app.rs`; the `Transfer` handle owns the channel. `Row` is the UI row type, aliased to avoid clashing with `transfer::Transfer`.)

- [ ] **Step 2: Replace signals and shared handles**

Replace the signal/handle block at the top of `App` (from `let (status, ...)` through the `pending` line) with:

```rust
    let (status, set_status) = signal(Status::Idle);
    let (room_link, set_room_link) = signal(String::new());
    let (room_code, set_room_code) = signal(String::new());
    let (items, set_items) = signal(Vec::<Row>::new());
    let (qr, set_qr) = signal(String::new());
    let (drag_depth, set_drag_depth) = signal(0i32);

    // Shared handles populated as the connection is established.
    let pc: Rc<RefCell<Option<RtcPeerConnection>>> = Rc::new(RefCell::new(None));
    let transfer: Rc<RefCell<Option<Transfer>>> = Rc::new(RefCell::new(None));
    let sig: Rc<RefCell<Option<Signaling>>> = Rc::new(RefCell::new(None));
    // Files chosen before the channel is open; their offers are sent on open.
    let pending: Rc<RefCell<Vec<web_sys::File>>> = Rc::new(RefCell::new(Vec::new()));
```

- [ ] **Step 3: Replace `upsert_progress`/`send_now` with row upserts and `Handlers`**

Replace the `upsert_progress` closure AND the `send_now` closure (everything from `// Helper to update one file's progress row.` up to but not including `// Wire a freshly-available data channel`) with:

```rust
    // Find-or-insert a transfer row keyed by (id, incoming), then mutate it.
    let upsert_row = move |id: u64,
                           incoming: bool,
                           make: &dyn Fn() -> Row,
                           apply: &dyn Fn(&mut Row)| {
        set_items.update(|list| {
            if let Some(row) = list.iter_mut().find(|r| r.id == id && r.incoming == incoming) {
                apply(row);
            } else {
                let mut row = make();
                apply(&mut row);
                list.push(row);
            }
        });
    };

    // Build the transfer event handlers (UI updates).
    let handlers = {
        let make_incoming = move |meta: &FileStart| Row {
            id: meta.id,
            name: meta.name.clone(),
            size: meta.size,
            kind: file_kind(&meta.name, &meta.mime),
            incoming: true,
            fraction: 0.0,
            state: TransferState::Offered,
        };
        Handlers {
            on_offer: Rc::new(move |meta: FileStart| {
                upsert_row(meta.id, true, &|| make_incoming(&meta), &|_r| {});
            }),
            on_recv_progress: Rc::new(move |id, name, recv, total| {
                let frac = if total > 0.0 { recv / total } else { 1.0 };
                upsert_row(
                    id,
                    true,
                    &|| Row {
                        id,
                        name: name.clone(),
                        size: total,
                        kind: file_kind(&name, ""),
                        incoming: true,
                        fraction: frac,
                        state: TransferState::Active,
                    },
                    &|r| {
                        r.fraction = frac;
                        r.state = TransferState::Active;
                    },
                );
            }),
            on_recv_complete: Rc::new(move |id, name| {
                upsert_row(
                    id,
                    true,
                    &|| Row {
                        id,
                        name: name.clone(),
                        size: 0.0,
                        kind: file_kind(&name, ""),
                        incoming: true,
                        fraction: 1.0,
                        state: TransferState::Done,
                    },
                    &|r| {
                        r.fraction = 1.0;
                        r.state = TransferState::Done;
                    },
                );
            }),
            on_send_progress: Rc::new(move |id, name, sent, total| {
                let frac = if total > 0.0 { sent / total } else { 1.0 };
                let done = frac >= 1.0;
                upsert_row(
                    id,
                    false,
                    &|| Row {
                        id,
                        name: name.clone(),
                        size: total,
                        kind: file_kind(&name, ""),
                        incoming: false,
                        fraction: frac,
                        state: if done { TransferState::Done } else { TransferState::Active },
                    },
                    &|r| {
                        r.fraction = frac;
                        r.state = if done { TransferState::Done } else { TransferState::Active };
                    },
                );
            }),
            on_rejected: Rc::new(move |id| {
                upsert_row(id, false, &|| Row {
                    id,
                    name: String::new(),
                    size: 0.0,
                    kind: "FILE",
                    incoming: false,
                    fraction: 0.0,
                    state: TransferState::Declined,
                }, &|r| r.state = TransferState::Declined);
            }),
        }
    };
```

> `upsert_row` captures `set_items` (Copy) and is itself `Copy`, so it can be moved into each handler closure. The `make`/`apply` params are `&dyn Fn` to keep the helper non-generic and reusable.

- [ ] **Step 4: Define an `offer_now` action and rewrite `wire_dc`**

Replace the entire `wire_dc` block with the following (which also defines `offer_now`, used by both `wire_dc`'s `onopen` flush and `on_files`):

```rust
    // Offer a batch of files now and add their outgoing rows.
    let offer_now: Rc<dyn Fn(Vec<web_sys::File>)> = {
        let transfer = transfer.clone();
        Rc::new(move |files: Vec<web_sys::File>| {
            let offered = transfer
                .borrow()
                .as_ref()
                .map(|t| t.offer_files(files))
                .unwrap_or_default();
            for (id, name, size) in offered {
                set_items.update(|list| {
                    list.push(Row {
                        id,
                        name: name.clone(),
                        size,
                        kind: file_kind(&name, ""),
                        incoming: false,
                        fraction: 0.0,
                        state: TransferState::Offered,
                    });
                });
            }
        })
    };

    // Wire a freshly-available data channel: build the Transfer + flush queue on open.
    let wire_dc = {
        let transfer = transfer.clone();
        let pending = pending.clone();
        let offer_now = offer_now.clone();
        let handlers = handlers.clone();
        Rc::new(move |channel: web_sys::RtcDataChannel| {
            let t = Transfer::new(channel, handlers.clone());
            // On open: mark connected and offer any files queued before connect.
            let pending = pending.clone();
            let offer_now = offer_now.clone();
            let onopen = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                set_status.set(Status::Connected);
                let queued: Vec<web_sys::File> = pending.borrow_mut().drain(..).collect();
                if !queued.is_empty() {
                    offer_now(queued);
                }
            });
            t.channel_set_onopen(onopen);
            *transfer.borrow_mut() = Some(t);
        })
    };
```

> `Transfer::new` installs `onmessage`. The `onopen` is set via a small helper on `Transfer` so the closure is owned/forgotten correctly. Add this method to `crates/client/src/transfer.rs` in the `impl Transfer` block:
>
> ```rust
>     /// Set the channel's `onopen` handler (takes ownership of the closure).
>     pub fn channel_set_onopen(&self, cb: wasm_bindgen::closure::Closure<dyn FnMut()>) {
>         self.shared.dc.set_onopen(Some(cb.as_ref().unchecked_ref()));
>         cb.forget();
>     }
> ```

- [ ] **Step 5: Update the Effect's data-channel wiring references**

In the mount `Effect`, the joiner/initiator branch still calls `wire_dc`. No change is needed there except removing the now-unused `dc_for_init` line. Replace:

```rust
            } else {
                // Initiator creates the channel up front.
                let channel = webrtc::create_data_channel(&peer);
                wire_dc(channel);
                let _ = &dc_for_init; // channel stored inside wire_dc
            }
```

with:

```rust
            } else {
                // Initiator creates the channel up front.
                let channel = webrtc::create_data_channel(&peer);
                wire_dc(channel);
            }
```

And remove the `let dc_for_init = dc.clone();` line earlier in the Effect's capture block (it referenced the removed `dc`).

- [ ] **Step 6: Rewrite `on_files`, accept/decline actions**

Replace the `on_files` closure with:

```rust
    // File-input/drop handler: offer immediately if open, else queue for on-open.
    let on_files = {
        let transfer = transfer.clone();
        let pending = pending.clone();
        let offer_now = offer_now.clone();
        move |files: Vec<web_sys::File>| {
            if files.is_empty() {
                return;
            }
            let open = transfer.borrow().as_ref().map(|t| t.is_open()).unwrap_or(false);
            if open {
                offer_now(files);
            } else {
                pending.borrow_mut().extend(files);
            }
        }
    };

    // Accept / decline an incoming offer.
    let on_accept = {
        let transfer = transfer.clone();
        move |id: u64| {
            if let Some(t) = transfer.borrow().as_ref() {
                t.accept(id);
            }
            // Optimistically leave the Offered state so the buttons hide and a
            // second click can't re-accept; real progress updates follow.
            set_items.update(|list| {
                if let Some(r) = list.iter_mut().find(|r| r.id == id && r.incoming) {
                    r.state = TransferState::Active;
                }
            });
        }
    };
    let on_decline = {
        let transfer = transfer.clone();
        move |id: u64| {
            if let Some(t) = transfer.borrow().as_ref() {
                t.reject(id);
            }
            // Remove the declined incoming row locally.
            set_items.update(|list| list.retain(|r| !(r.id == id && r.incoming)));
        }
    };
    let on_accept_all = {
        let transfer = transfer.clone();
        move || {
            let ids: Vec<u64> = items
                .get_untracked()
                .iter()
                .filter(|r| r.incoming && r.state == TransferState::Offered)
                .map(|r| r.id)
                .collect();
            if let Some(t) = transfer.borrow().as_ref() {
                for id in &ids {
                    t.accept(*id);
                }
            }
            set_items.update(|list| {
                for r in list.iter_mut() {
                    if r.incoming && ids.contains(&r.id) {
                        r.state = TransferState::Active;
                    }
                }
            });
        }
    };
```

- [ ] **Step 7: Update the view**

In the final `view!`, replace the `<ProgressList items=progress/>` line with:

```rust
            <ProgressList
                items=items
                on_accept=Callback::new(on_accept)
                on_decline=Callback::new(on_decline)
                on_accept_all=Callback::new(move |_| on_accept_all())
            />
```

(The `<StatusBar>`, `<ShareLink>`, `<JoinBox>`, wordmark, and drop zone lines are unchanged. `Callback` is from `leptos::prelude::*`.)

- [ ] **Step 8: Add CSS for the new states**

In `crates/client/styles.css`, append:

```css
/* Transfer offer / decline / actions */
.acceptall {
  border: 3px solid var(--ink); background: var(--orange); color: var(--ink);
  font-family: "Archivo Black", sans-serif; font-size: 12px;
  text-transform: uppercase; padding: 6px 14px; margin-bottom: 10px; cursor: pointer;
}
.row.offer .actions { display: flex; gap: 8px; }
.row .accept, .row .decline {
  border: 2px solid var(--ink); font-family: "Space Grotesk", sans-serif;
  font-weight: 700; font-size: 11px; text-transform: uppercase; padding: 4px 10px; cursor: pointer;
}
.row .accept { background: var(--orange); color: var(--ink); }
.row .decline { background: var(--white); color: var(--ink); }
.row .size { font-family: "Space Mono", monospace; font-size: 10px; opacity: 0.7; }
.row.waiting .pct, .row.declined .pct {
  font-family: "Space Mono", monospace; font-size: 11px; opacity: 0.7;
}
.row.declined { opacity: 0.6; }
```

- [ ] **Step 9: Build the whole client**

Run: `cd crates/client && trunk build`
Expected: success, no errors. Fix any compile errors (likely closure `move`/clone or `IntoAny` issues). The pre-existing `protocol.rs::chunk_bytes` dead-code warning may appear and is acceptable.

- [ ] **Step 10: Run unit tests**

Run: `cargo test -p client`
Expected: PASS — protocol roundtrips (incl. Offer/Accept/Reject), `fmt_size`, `file_kind`, `qr_svg`, `normalize_code`, `resolve_origin`.

- [ ] **Step 11: Commit (Tasks 2, 3, 4 together)**

```bash
git add crates/client/src/ui.rs crates/client/src/transfer.rs crates/client/src/app.rs crates/client/styles.css
git commit -m "feat(client): accept-before-transfer handshake with per-file accept/decline"
```

End the commit body with:
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

---

## Task 5: Browser verification

**Files:** none (verification only). Use the chrome-devtools MCP or `/run`.

- [ ] **Step 1: Build release and start the server**

```bash
cd crates/client && trunk build --release && cd ../..
cargo run --release -p server
```

Open `http://localhost:3000` (host) and a second tab via the host's code (joiner).

- [ ] **Step 2: Verify the checklist**

- Host (or joiner) chooses a file → an **outgoing row** shows "WAITING…"; the other side shows an **incoming offer** row with the file name, size, and **Accept**/**Decline** buttons. Confirm (via DevTools network or the absence of a progress bar) that **no bytes/`Start`** were sent yet.
- Click **Accept** → the file transfers; sender row shows `↑ … %` then `✓ DONE`; receiver row shows `↓ … %` then `✓ DONE`; the file downloads.
- Send another file and click **Decline** → sender row shows `✗ DECLINED`; no download on the receiver; the incoming row disappears on the decliner.
- Send 2+ files at once → both incoming offers appear; **Accept all** accepts both; each transfers.
- Repeat in the **other direction** (joiner→host).
- Choose a file **before** the peer connects → after the peer joins, the offer appears (queued offer flushed on open).

- [ ] **Step 3: Final commit if anything changed**

If verification surfaced a fix, commit it. Otherwise note "verification only, no changes."

---

## Self-Review

**Spec coverage:**
- Protocol Offer/Accept/Reject → Task 1.
- No bytes until accept (sender offers, streams only on Accept) → Task 3 (`offer_files` sends only `Offer`; `on_accept`/`drain` stream on Accept).
- Per-file Accept/Decline → Task 2 (`transfer_row` buttons) + Task 4 (`on_accept`/`on_decline`).
- Accept all → Task 2 (button, shown when ≥2 pending incoming) + Task 4 (`on_accept_all`).
- Sequential streaming, one active incoming → Task 3 (`drain` queue + single `sending` flag; `Incoming` single-file).
- Session-unique ids → Task 3 (`Shared.next_id`).
- Unified router (both roles) → Task 3 (`install_router`).
- UI states Offered/Active/Done/Declined → Task 2 (`TransferState`, `transfer_row`).
- Pre-connect queue offers on open → Task 4 (`offer_now` from `wire_dc` onopen).
- Decline shows declined; batch continues → Task 3 (`on_rejected`) + Task 4 (`on_rejected` handler) + Task 2 (declined row).
- Unknown control ignored, 0-byte files → Task 3 (router `None`/unmatched are inert; `drain` final 100% covers 0-byte).
- Testing: protocol roundtrips + `fmt_size` unit; browser checklist → Tasks 1, 2, 5.

**Placeholder scan:** No "TBD"/"add error handling"/"similar to". Every code step shows full code. The `channel_set_onopen` helper is fully specified inline in Task 4 Step 4.

**Type consistency:** `Row` = `ui::Transfer { id, name, size, kind, incoming, fraction, state }` is constructed identically across all Task 4 handlers and Task 2. `Handlers` field signatures (`on_offer: Fn(FileStart)`, `on_recv_progress: Fn(u64,String,f64,f64)`, `on_recv_complete: Fn(u64,String)`, `on_send_progress: Fn(u64,String,f64,f64)`, `on_rejected: Fn(u64)`) match between Task 3's definition and Task 4's construction. `Transfer` methods used in Task 4 (`new`, `is_open`, `offer_files`, `accept`, `reject`, `channel_set_onopen`) all exist in Task 3. `ProgressList` props (`items`, `on_accept`, `on_decline`, `on_accept_all`) match between Task 2 and Task 4. `Control::Offer/Accept/Reject` from Task 1 are used by Task 3.
