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

    /// Set the channel's `onopen` handler (takes ownership of the closure).
    pub fn channel_set_onopen(&self, cb: wasm_bindgen::closure::Closure<dyn FnMut()>) {
        self.shared.dc.set_onopen(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
}

/// Install the unified control/message router on the channel.
fn install_router(shared: &Rc<Shared>) {
    let shared_for_cb = shared.clone();
    let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        let shared = &shared_for_cb;
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
                Some(Control::End(_)) => finalize(shared),
                // Sender role.
                Some(Control::Accept { id }) => on_accept(shared, id),
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
