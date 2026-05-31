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
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    Blob, File, MessageEvent, RtcDataChannel, RtcDataChannelState, RtcPeerConnection, Url,
};

use crate::protocol::{decode_control, encode_control, Control, FileEnd, FileStart, CHUNK_SIZE};

/// High-water mark for buffered bytes on a file channel; pause sending above it.
const BUFFER_HIGH: f64 = 1_000_000.0;

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
}

/// Receiver state for one incoming file. A local cancel removes the whole entry
/// (see [`Transfer::cancel`]), so there is no per-file cancelled flag: a missing
/// entry is the signal to ignore any bytes still arriving on the wire.
#[derive(Default)]
struct Incoming {
    meta: Option<FileStart>,
    chunks: Vec<js_sys::Uint8Array>,
    received: f64,
    /// Rolling-window speed estimate (bytes/sec) and its accumulators.
    speed: f64,
    win_start: f64, // performance.now() at window start; 0.0 = unset
    win_bytes: f64, // bytes received within the current window
}

/// Sender state: offered files awaiting a decision, the accepted-but-not-yet-
/// started queue, the set of files currently streaming, and ids the peer
/// cancelled.
///
/// Generic over the file payload `F` so the pure state transitions can be
/// exercised in tests without a browser `File` (production uses `Outgoing<File>`).
struct Outgoing<F> {
    offered: HashMap<u64, F>,
    queue: VecDeque<(u64, F)>,
    active: HashSet<u64>,
    cancelled: HashSet<u64>,
}

// Hand-written so it doesn't require `F: Default` (a `web_sys::File` has no Default).
impl<F> Default for Outgoing<F> {
    fn default() -> Self {
        Outgoing {
            offered: HashMap::new(),
            queue: VecDeque::new(),
            active: HashSet::new(),
            cancelled: HashSet::new(),
        }
    }
}

/// Sender: an offered file `id` was accepted — move it to the send queue.
fn enqueue_accepted<F>(out: &mut Outgoing<F>, id: u64) {
    if let Some(file) = out.offered.remove(&id) {
        out.queue.push_back((id, file));
    }
}

/// Sender: start queued files until `active` reaches `max`, marking each
/// started file active. Returns the files the caller must begin streaming.
fn schedule<F>(out: &mut Outgoing<F>, max: usize) -> Vec<(u64, F)> {
    let mut started = Vec::new();
    while out.active.len() < max {
        let Some((id, file)) = out.queue.pop_front() else { break };
        out.active.insert(id);
        started.push((id, file));
    }
    started
}

/// Sender: the peer cancelled file `id`. Drop it from the offered set and queue
/// and flag it so an in-progress `send_file` stops streaming.
fn cancel_outgoing<F>(out: &mut Outgoing<F>, id: u64) {
    out.offered.remove(&id);
    out.queue.retain(|(qid, _)| *qid != id);
    out.cancelled.insert(id);
}

/// Receiver: fold a freshly-received chunk of `len` bytes into the incoming
/// state and return progress `(id, name, size, received, speed)` if a transfer
/// is in flight. Assumes the caller has already checked it isn't cancelled.
fn account_chunk(inc: &mut Incoming, len: f64, now: f64) -> Option<(u64, String, f64, f64, f64)> {
    inc.received += len;
    update_speed(inc, len, now);
    inc.meta
        .as_ref()
        .map(|m| (m.id, m.name.clone(), m.size, inc.received, inc.speed))
}

/// Receiver: consume the incoming state at end-of-file. Returns the file meta to
/// save, or `None` if no transfer ever started on this channel.
fn finalize_decision(inc: &mut Incoming) -> Option<FileStart> {
    inc.meta.take()
}

/// State shared between the control router, per-file receive routers, and the
/// async sender tasks.
struct Shared {
    pc: RtcPeerConnection,
    ctrl: RtcDataChannel,
    handlers: Handlers,
    next_id: Cell<u64>,
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
            next_id: Cell::new(0),
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
            let _ = self.shared.ctrl.send_with_str(&encode_control(&Control::Offer(meta.clone())));
            offered.push((id, meta.name.clone(), meta.size));
            self.shared.outgoing.borrow_mut().offered.insert(id, file);
        }
        offered
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

/// Fold `len` new bytes into the rolling-window speed estimate. The window
/// closes every ~250ms, smoothing out per-chunk jitter into a stable rate.
fn update_speed(inc: &mut Incoming, len: f64, now: f64) {
    const WINDOW_MS: f64 = 250.0;
    inc.win_bytes += len;
    if inc.win_start == 0.0 {
        inc.win_start = now;
        return;
    }
    let dt = now - inc.win_start;
    if dt >= WINDOW_MS {
        inc.speed = inc.win_bytes / (dt / 1000.0);
        inc.win_start = now;
        inc.win_bytes = 0.0;
    }
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
    let _ = send_file(dc.clone(), id, file, prog, &is_cancelled).await;
    if is_cancelled() {
        // Peer aborted mid-stream; the UI is already marked cancelled.
        shared.outgoing.borrow_mut().cancelled.remove(&id);
    } else {
        // Guarantee a final 100% (also covers 0-byte files that emit no chunks).
        (shared.handlers.on_send_progress)(id, name, total, total);
    }
    dc.close();

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

/// Send one file on its channel: `Start` -> chunks (with backpressure) -> `End`.
async fn send_file(
    dc: RtcDataChannel,
    id: u64,
    file: File,
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

    let mut offset: f64 = 0.0;
    let mut sent: f64 = 0.0;
    while offset < size {
        // Stop early if the receiver cancelled; no `End` is sent.
        if is_cancelled() {
            return Ok(());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: u64) -> FileStart {
        FileStart { id, name: format!("f{id}"), size: 100.0, mime: String::new() }
    }

    fn incoming_with(meta: FileStart) -> Incoming {
        Incoming { meta: Some(meta), ..Incoming::default() }
    }

    // --- sender scheduling (Outgoing) ---

    #[test]
    fn schedule_starts_up_to_the_cap() {
        let mut out: Outgoing<u32> = Outgoing::default();
        for id in 0..6u64 {
            out.offered.insert(id, id as u32);
            enqueue_accepted(&mut out, id);
        }
        let started = schedule(&mut out, 4);
        assert_eq!(started.len(), 4); // capped
        assert_eq!(out.active.len(), 4);
        assert_eq!(out.queue.len(), 2); // remainder waits
    }

    #[test]
    fn schedule_resumes_as_slots_free() {
        let mut out: Outgoing<u32> = Outgoing::default();
        for id in 0..5u64 {
            out.offered.insert(id, id as u32);
            enqueue_accepted(&mut out, id);
        }
        let first = schedule(&mut out, 2);
        assert_eq!(first.len(), 2);
        // A slot frees when one finishes; the next queued file starts.
        out.active.remove(&first[0].0);
        let next = schedule(&mut out, 2);
        assert_eq!(next.len(), 1);
        assert_eq!(out.active.len(), 2);
    }

    #[test]
    fn enqueue_accepted_ignores_unknown_id() {
        let mut out: Outgoing<u32> = Outgoing::default();
        enqueue_accepted(&mut out, 99);
        assert!(out.queue.is_empty());
    }

    #[test]
    fn cancel_removes_from_offered_queue_and_flags() {
        let mut out: Outgoing<u32> = Outgoing::default();
        out.offered.insert(1, 111);
        out.queue.push_back((2, 222));
        cancel_outgoing(&mut out, 1);
        cancel_outgoing(&mut out, 2);
        assert!(out.offered.is_empty());
        assert!(out.queue.is_empty());
        assert!(out.cancelled.contains(&1));
        assert!(out.cancelled.contains(&2));
    }

    #[test]
    fn cancelled_active_file_is_not_rescheduled() {
        let mut out: Outgoing<u32> = Outgoing::default();
        out.offered.insert(1, 111);
        enqueue_accepted(&mut out, 1);
        let started = schedule(&mut out, 4);
        assert_eq!(started.len(), 1);
        // Peer cancels the in-flight file; nothing new is queued to start.
        cancel_outgoing(&mut out, 1);
        assert!(schedule(&mut out, 4).is_empty());
    }

    // --- receiver chunk accounting ---

    #[test]
    fn account_chunk_tracks_received_and_reports_progress() {
        let mut inc = incoming_with(meta(7));
        let p = account_chunk(&mut inc, 40.0, 0.0).unwrap();
        assert_eq!(p.0, 7); // id
        assert_eq!(p.2, 100.0); // size
        assert_eq!(p.3, 40.0); // received
        let p2 = account_chunk(&mut inc, 60.0, 0.0).unwrap();
        assert_eq!(p2.3, 100.0);
    }

    #[test]
    fn account_chunk_without_meta_yields_no_progress() {
        let mut inc = Incoming::default();
        assert!(account_chunk(&mut inc, 10.0, 0.0).is_none());
        assert_eq!(inc.received, 10.0); // bytes still counted
    }

    // --- finalize decision ---

    #[test]
    fn finalize_decision_returns_meta() {
        let mut inc = incoming_with(meta(9));
        let m = finalize_decision(&mut inc).unwrap();
        assert_eq!(m.id, 9);
        assert!(inc.meta.is_none());
    }

    #[test]
    fn finalize_decision_with_no_transfer_is_none() {
        let mut inc = Incoming::default();
        assert!(finalize_decision(&mut inc).is_none());
    }

    // --- rolling-window speed estimate ---

    #[test]
    fn update_speed_closes_window_after_threshold() {
        let mut inc = Incoming::default();
        // First sample only seeds the window start; speed stays unknown.
        update_speed(&mut inc, 1000.0, 1000.0);
        assert_eq!(inc.speed, 0.0);
        // 300ms later (>= 250ms window): 2000 bytes over 0.3s.
        update_speed(&mut inc, 1000.0, 1300.0);
        assert!((inc.speed - 2000.0 / 0.3).abs() < 1.0);
        assert_eq!(inc.win_bytes, 0.0); // window reset
    }

    #[test]
    fn update_speed_holds_within_window() {
        let mut inc = Incoming::default();
        update_speed(&mut inc, 500.0, 1000.0);
        update_speed(&mut inc, 500.0, 1100.0); // only 100ms elapsed
        assert_eq!(inc.speed, 0.0);
        assert_eq!(inc.win_bytes, 1000.0);
    }
}
