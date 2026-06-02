//! Parallel file transfer with an accept-before-transfer handshake.
//!
//! Signaling and bulk bytes are split across channels:
//!
//! * A single **control channel** (label [`CTRL_LABEL`]) carries the handshake:
//!   the sender announces each file with `Offer` (no bytes); the receiver replies
//!   `Accept{id}` / `Reject{id}`, and may `Cancel{id}` mid-transfer.
//! * Each **accepted file gets its own data channel** (label = the file id). The
//!   sender streams `Start` -> binary chunks -> `End` on it and then closes it.
//!   Because additional data channels are multiplexed over the already-negotiated
//!   SCTP transport, several files transfer concurrently — bounded by
//!   [`MAX_CONCURRENT`] so a large batch doesn't open unbounded channels at once.
//!
//! The receiver keys in-flight files by id so multiple reassemble in parallel.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    Blob, File, MessageEvent, RtcDataChannel, RtcDataChannelState, RtcPeerConnection, Url,
};

use crate::protocol::{
    decode_control, encode_control, split_ranges, Control, FileEnd, FileStart, CHUNK_SIZE,
    SLAB_SIZE,
};
use crate::transfer_state::{
    account_chunk, cancel_outgoing, enqueue_accepted, finalize_decision, schedule, Incoming,
    Outgoing,
};

/// Pause sending once this many bytes sit unsent in the channel's SCTP buffer.
/// Well under Chrome's ~16 MB send-buffer ceiling (which throws if exceeded).
const BUFFER_HIGH: f64 = 8.0 * 1024.0 * 1024.0;

/// Resume sending once the buffer drains below this (the `bufferedamountlow`
/// threshold). The gap to [`BUFFER_HIGH`] is hysteresis: a wide gap means we
/// pause rarely and keep the pipe full instead of stalling per chunk.
const BUFFER_LOW: u32 = 1024 * 1024;

/// Maximum number of files streaming at once. Further accepts queue.
const MAX_CONCURRENT: usize = 4;

/// Label of the control channel; every other channel's label is a file id.
pub const CTRL_LABEL: &str = "ctrl";

/// UI callbacks for transfer events. All are `Rc<dyn Fn..>` so they can be
/// cloned into the message routers and the async sender tasks.
#[derive(Clone)]
pub struct Handlers {
    /// An incoming file was offered; show an accept/decline prompt.
    pub on_offer: Rc<dyn Fn(FileStart)>,
    /// Receiving progress for incoming file `id`: (id, name, received, total, bytes_per_sec).
    pub on_recv_progress: Rc<dyn Fn(u64, String, f64, f64, f64)>,
    /// Incoming file `id` finished and was saved: (id, name).
    pub on_recv_complete: Rc<dyn Fn(u64, String)>,
    /// Sending progress for outgoing file `id`: (id, name, sent, total).
    pub on_send_progress: Rc<dyn Fn(u64, String, f64, f64)>,
    /// Outgoing file `id` was declined by the peer.
    pub on_rejected: Rc<dyn Fn(u64)>,
    /// Outgoing file `id` was cancelled mid-transfer by the peer.
    pub on_cancelled: Rc<dyn Fn(u64)>,
    /// Incoming file `id` was aborted by the sender; discard the partial download.
    pub on_aborted: Rc<dyn Fn(u64)>,
}

/// State shared between the control router, per-file receive routers, and the
/// async sender tasks.
struct Shared {
    pc: RtcPeerConnection,
    ctrl: RtcDataChannel,
    handlers: Handlers,
    incoming: RefCell<HashMap<u64, Incoming>>,
    outgoing: RefCell<Outgoing<File>>,
}

/// Handle to a peer connection wired for the parallel transfer protocol.
#[derive(Clone)]
pub struct Transfer {
    shared: Rc<Shared>,
}

impl Transfer {
    /// Wrap the control channel `ctrl` on peer connection `pc`: install the
    /// control router and return a handle. Per-file channels are created on
    /// demand from `pc` as files are accepted.
    pub fn new(pc: RtcPeerConnection, ctrl: RtcDataChannel, handlers: Handlers) -> Transfer {
        let shared = Rc::new(Shared {
            pc,
            ctrl,
            handlers,
            incoming: RefCell::new(HashMap::new()),
            outgoing: RefCell::new(Outgoing::default()),
        });
        install_ctrl_router(&shared);
        Transfer { shared }
    }

    /// Is the control channel open?
    pub fn is_open(&self) -> bool {
        self.shared.ctrl.ready_state() == RtcDataChannelState::Open
    }

    /// Announce already-id'd `files` to the peer (one `Offer` each). The caller
    /// assigns ids up front (so a file can be shown the moment it is added, before
    /// the channel is open); this sends the offers and remembers each file for when
    /// the peer accepts.
    pub fn offer_files(&self, files: Vec<(u64, File)>) {
        for (id, file) in files {
            let meta = FileStart {
                id,
                name: file.name(),
                size: file.size(),
                mime: file.type_(),
            };
            let _ = self.shared.ctrl.send_with_str(&encode_control(&Control::Offer(meta)));
            self.shared.outgoing.borrow_mut().offered.insert(id, file);
        }
    }

    /// Accept an incoming offered file (sends `Accept{id}` to the sender).
    pub fn accept(&self, id: u64) {
        let _ = self.shared.ctrl.send_with_str(&encode_control(&Control::Accept { id }));
    }

    /// Decline an incoming offered file (sends `Reject{id}`).
    pub fn reject(&self, id: u64) {
        let _ = self.shared.ctrl.send_with_str(&encode_control(&Control::Reject { id }));
    }

    /// Cancel the in-progress incoming file `id`: tell the sender to stop and
    /// discard what we've buffered so far (no download is triggered).
    pub fn cancel(&self, id: u64) {
        let _ = self.shared.ctrl.send_with_str(&encode_control(&Control::Cancel { id }));
        // Drop the partial download. The sender stops without an `End`, so the
        // entry would otherwise linger; removing it also makes any late chunk on
        // the wire a no-op (the chunk path ignores ids absent from the map).
        self.shared.incoming.borrow_mut().remove(&id);
    }

    /// Abort the in-progress outgoing file `id` (sender-initiated): flag it so the
    /// streaming task stops (and closes its channel), and tell the receiver to
    /// discard the partial download. The mirror of [`cancel`] for the send side.
    pub fn abort(&self, id: u64) {
        cancel_outgoing(&mut self.shared.outgoing.borrow_mut(), id);
        let _ = self.shared.ctrl.send_with_str(&encode_control(&Control::Abort { id }));
    }

    /// Set the control channel's `onopen` handler (takes ownership of the closure).
    pub fn channel_set_onopen(&self, cb: wasm_bindgen::closure::Closure<dyn FnMut()>) {
        self.shared.ctrl.set_onopen(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }

    /// Wire an inbound per-file data channel (its label is the file id): install
    /// a receive router that reassembles that one file.
    pub fn handle_incoming_channel(&self, dc: RtcDataChannel) {
        install_recv_router(&self.shared, dc);
    }
}

/// Install the control router: handshake frames only (no bytes flow here).
fn install_ctrl_router(shared: &Rc<Shared>) {
    let shared_for_cb = shared.clone();
    let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        let shared = &shared_for_cb;
        let Some(text) = e.data().as_string() else { return };
        match decode_control(&text) {
            Some(Control::Offer(meta)) => (shared.handlers.on_offer)(meta),
            Some(Control::Accept { id }) => {
                // Queue the accepted file, then start as many as the cap allows.
                let started = {
                    let mut out = shared.outgoing.borrow_mut();
                    enqueue_accepted(&mut out, id);
                    schedule(&mut out, MAX_CONCURRENT)
                };
                for (sid, file) in started {
                    spawn_local(send_file_on_channel(shared.clone(), sid, file));
                }
            }
            Some(Control::Reject { id }) => {
                shared.outgoing.borrow_mut().offered.remove(&id);
                (shared.handlers.on_rejected)(id);
            }
            Some(Control::Cancel { id }) => {
                cancel_outgoing(&mut shared.outgoing.borrow_mut(), id);
                (shared.handlers.on_cancelled)(id);
            }
            Some(Control::Abort { id }) => {
                // The sender aborted a file it was streaming to us: drop the
                // partial download so any late chunk is ignored, then mark the row.
                shared.incoming.borrow_mut().remove(&id);
                (shared.handlers.on_aborted)(id);
            }
            // Start/End never travel on the control channel.
            _ => {}
        }
    });
    shared.ctrl.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
}

/// Install a receive router on a per-file channel. The channel carries exactly
/// one file: `Start` (meta) -> binary chunks -> `End`.
fn install_recv_router(shared: &Rc<Shared>, dc: RtcDataChannel) {
    dc.set_binary_type(web_sys::RtcDataChannelType::Arraybuffer);
    // The file id for this channel, learned from its `Start` frame.
    let cur_id: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
    let shared_for_cb = shared.clone();
    let cur = cur_id.clone();
    // Closing the channel is the receiver's job (see the `End` arm below), so the
    // message handler needs its own handle to the channel.
    let dc_for_close = dc.clone();
    let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        let shared = &shared_for_cb;
        let data = e.data();
        if let Some(text) = data.as_string() {
            match decode_control(&text) {
                Some(Control::Start(meta)) => {
                    let id = meta.id;
                    cur.set(Some(id));
                    shared
                        .incoming
                        .borrow_mut()
                        .entry(id)
                        .or_default()
                        .meta
                        .get_or_insert(meta);
                }
                Some(Control::End(_)) => {
                    if let Some(id) = cur.get() {
                        finalize_recv(shared, id);
                    }
                    // The receiver now holds every byte: `End` follows all chunks,
                    // and SCTP delivers a stream in order, so its arrival proves the
                    // chunks arrived. Close from this side. The sender must NOT close
                    // right after its last `send()` — that resets the SCTP stream
                    // while the just-queued bytes may still be in flight, and Chrome
                    // discards them, so a small/fast file lands as 0 bytes. Resetting
                    // from the side that already has the data carries no such risk.
                    dc_for_close.close();
                }
                _ => {}
            }
        } else {
            // Binary chunk for this channel's file.
            let Some(id) = cur.get() else { return };
            let array = js_sys::Uint8Array::new(&data);
            let progress = {
                let mut map = shared.incoming.borrow_mut();
                // A missing entry means the file was cancelled; ignore the bytes.
                let Some(inc) = map.get_mut(&id) else { return };
                let len = array.length() as f64;
                inc.chunks.push(array);
                account_chunk(inc, len, now_ms())
            };
            if let Some((id, name, total, recv, speed)) = progress {
                (shared.handlers.on_recv_progress)(id, name, recv, total, speed);
            }
        }
    });
    dc.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
}

/// Current high-resolution time in milliseconds, or 0.0 if unavailable.
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

/// Sender: open a dedicated channel for file `id`, stream it, then close the
/// channel. When it finishes, free its concurrency slot and pump the queue.
async fn send_file_on_channel(shared: Rc<Shared>, id: u64, file: File) {
    let dc = shared.pc.create_data_channel(&id.to_string());
    dc.set_binary_type(web_sys::RtcDataChannelType::Arraybuffer);
    await_channel_open(&dc).await;

    let name = file.name();
    let total = file.size();
    let prog = {
        let shared = shared.clone();
        let name = name.clone();
        move |sent: f64| (shared.handlers.on_send_progress)(id, name.clone(), sent, total)
    };
    let is_cancelled = {
        let shared = shared.clone();
        move || shared.outgoing.borrow().cancelled.contains(&id)
    };
    // Never hand SCTP a message larger than the negotiated maxMessageSize.
    let chunk = max_message_size(&shared.pc).min(CHUNK_SIZE);
    let _ = send_file(dc.clone(), id, file, chunk, prog, &is_cancelled).await;
    if is_cancelled() {
        // Peer aborted mid-stream; the UI is already marked cancelled. The receiver
        // is discarding this file, so close from here — losing the bytes still in
        // the SCTP buffer is exactly what cancelling means.
        shared.outgoing.borrow_mut().cancelled.remove(&id);
        dc.close();
    } else {
        // Guarantee a final 100% (also covers 0-byte files that emit no chunks).
        (shared.handlers.on_send_progress)(id, name, total, total);
        // Do NOT close here. Closing right after the last `send()` resets the SCTP
        // stream while the just-queued bytes may not yet be on the wire, and Chrome
        // discards them — a small/fast file then arrives as 0 bytes. The receiver
        // closes the channel once it has the file (on `End`); see install_recv_router.
    }

    // Free this slot and start any files waiting on the concurrency cap.
    let next = {
        let mut out = shared.outgoing.borrow_mut();
        out.active.remove(&id);
        schedule(&mut out, MAX_CONCURRENT)
    };
    for (sid, f) in next {
        spawn_local(send_file_on_channel(shared.clone(), sid, f));
    }
}

/// Resolve once `dc` reaches the open state.
async fn await_channel_open(dc: &RtcDataChannel) {
    if dc.ready_state() == RtcDataChannelState::Open {
        return;
    }
    let dc2 = dc.clone();
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let cb = Closure::once(Box::new(move || {
            let _ = resolve.call0(&JsValue::NULL);
        }) as Box<dyn FnOnce()>);
        dc2.set_onopen(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    });
    let _ = JsFuture::from(promise).await;
}

/// The channel's negotiated SCTP `maxMessageSize`, or [`usize::MAX`] when it's
/// unavailable or unbounded. Read via reflection (`pc.sctp.maxMessageSize`)
/// because web-sys 0.3 has no `RtcSctpTransport` binding. The SCTP transport
/// (and thus this limit) exists once the connection is up, which it is by the
/// time we stream a file.
fn max_message_size(pc: &RtcPeerConnection) -> usize {
    let mms = js_sys::Reflect::get(pc, &JsValue::from_str("sctp"))
        .ok()
        .filter(|sctp| !sctp.is_null() && !sctp.is_undefined())
        .and_then(|sctp| js_sys::Reflect::get(&sctp, &JsValue::from_str("maxMessageSize")).ok())
        .and_then(|v| v.as_f64());
    // A non-finite value means "no limit"; 0 has historically meant the same.
    // Either way fall back to unbounded and let CHUNK_SIZE govern.
    match mms {
        Some(m) if m.is_finite() && m >= 1.0 => m as usize,
        _ => usize::MAX,
    }
}

/// Send one file on its channel: `Start` -> chunks (with backpressure) -> `End`.
///
/// Reads the file in [`SLAB_SIZE`] slabs and slices each into `chunk`-sized
/// messages in memory, prefetching the next slab while the current one streams
/// so reads overlap sends. Sending paces on the `bufferedamountlow` event
/// (see [`await_buffered_low`]) rather than a timer, which would be throttled in
/// a background tab.
async fn send_file(
    dc: RtcDataChannel,
    id: u64,
    file: File,
    chunk: usize,
    on_progress: impl Fn(f64) + 'static,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), JsValue> {
    let size = file.size();
    let start = Control::Start(FileStart {
        id,
        name: file.name(),
        size,
        mime: file.type_(),
    });
    dc.send_with_str(&encode_control(&start))?;
    // Fire `bufferedamountlow` once the buffer drains below this so we can pace
    // sending on a network event rather than a timer (timers are throttled to
    // ~1/sec in background tabs, which collapses throughput).
    dc.set_buffered_amount_low_threshold(BUFFER_LOW);

    // Start an `arrayBuffer()` read for one slab. The browser fills it in the
    // background, so the next slab can be read while the current one is still
    // being sent — overlapping file I/O with sending instead of stalling on
    // each read (reads were ~50% of wall time when serialized).
    let read_slab = |range: (u64, u64)| -> Result<JsFuture, JsValue> {
        let blob = file.slice_with_f64_and_f64(range.0 as f64, range.1 as f64)?;
        Ok(JsFuture::from(blob.array_buffer()))
    };
    let ranges = split_ranges(size as u64, SLAB_SIZE);
    let mut idx = 0usize;
    // Prime the pipeline: kick off the first read before the loop.
    let mut pending = match ranges.first() {
        Some(&r) => Some(read_slab(r)?),
        None => None,
    };

    let mut sent: f64 = 0.0;
    while let Some(fut) = pending.take() {
        // Stop early if the receiver cancelled; no `End` is sent.
        if is_cancelled() {
            return Ok(());
        }
        let buf = fut.await?;
        let slab = js_sys::Uint8Array::new(&buf);
        let slab_len = slab.length();

        // Kick off the next slab's read now so it overlaps the sends below.
        idx += 1;
        if let Some(&r) = ranges.get(idx) {
            pending = Some(read_slab(r)?);
        }

        let mut off: u32 = 0;
        while off < slab_len {
            if is_cancelled() {
                return Ok(());
            }
            let end = (off + chunk as u32).min(slab_len);
            // `subarray` is a zero-copy view into the slab; the send copies it
            // into the SCTP buffer. (Using `.buffer()` here would send the whole
            // slab, not the chunk — the view is what bounds the message size.)
            let view = slab.subarray(off, end);

            // Wait for the buffer to drain via the bufferedamountlow event, not a
            // timer. (Synchronous code can't change buffered_amount mid-check, so
            // there's no lost-wakeup race between the test and the await.)
            if dc.buffered_amount() as f64 > BUFFER_HIGH {
                await_buffered_low(&dc).await;
                // A `Cancel` may have arrived while parked on the await. Re-check
                // before sending/reporting another chunk: otherwise this iteration's
                // `on_progress` would overwrite the `Cancelled` row back to `Active`,
                // freezing the sender's UI mid-progress instead of showing cancelled.
                if is_cancelled() {
                    return Ok(());
                }
            }

            dc.send_with_array_buffer_view(&view)?;
            sent += (end - off) as f64;
            off = end;
            on_progress(sent);
        }
    }

    dc.send_with_str(&encode_control(&Control::End(FileEnd { id })))?;
    Ok(())
}

/// Resolve once the channel's buffered amount drains below its
/// `bufferedAmountLowThreshold`. Driven by the `bufferedamountlow` event, which
/// the network stack fires on drain — unlike `setTimeout`, it is not throttled
/// in background tabs, so sending keeps pace whether or not the tab is focused.
async fn await_buffered_low(dc: &RtcDataChannel) {
    let dc2 = dc.clone();
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let dc3 = dc2.clone();
        let cb = Closure::once(Box::new(move || {
            dc3.set_onbufferedamountlow(None);
            let _ = resolve.call0(&JsValue::NULL);
        }) as Box<dyn FnOnce()>);
        dc2.set_onbufferedamountlow(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    });
    let _ = JsFuture::from(promise).await;
}

/// Yield control to the browser event loop for one timer tick.
async fn yield_to_event_loop() {
    let _ = JsFuture::from(js_sys::Promise::new(&mut |resolve, _| {
        let window = web_sys::window().unwrap();
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
    }))
    .await;
}

/// Assemble a finished file's chunks into a Blob and trigger a browser download.
fn finalize_recv(shared: &Rc<Shared>, id: u64) {
    let (meta, parts) = {
        let mut map = shared.incoming.borrow_mut();
        let Some(inc) = map.get_mut(&id) else { return };
        let parts = std::mem::take(&mut inc.chunks);
        // None for a cancelled (raced an in-flight `End`) or never-started file,
        // so the partial file is dropped silently.
        let meta = finalize_decision(inc);
        map.remove(&id);
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
