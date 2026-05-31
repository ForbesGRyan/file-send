//! File send (chunk + backpressure) and receive (reassemble + download).

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Blob, File, MessageEvent, RtcDataChannel, Url};

use crate::protocol::{decode_control, encode_control, Control, FileEnd, FileStart, CHUNK_SIZE};

/// High-water mark for buffered bytes; pause sending above this.
const BUFFER_HIGH: f64 = 1_000_000.0;

/// Send one file over the data channel, reporting progress as bytes sent.
/// `id` must be unique per file in the session.
pub async fn send_file(
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
        // Slice the file lazily; only this chunk is read into memory.
        let blob = file.slice_with_f64_and_f64(offset, end)?;
        let buf = JsFuture::from(blob.array_buffer()).await?;
        let array = js_sys::Uint8Array::new(&buf);

        // Backpressure: wait while the channel's send buffer is too full.
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

/// State accumulated while receiving the current file.
#[derive(Default)]
struct Incoming {
    meta: Option<FileStart>,
    chunks: Vec<js_sys::Uint8Array>,
    received: f64,
}

/// Attach receive handling to a data channel. `on_progress(name, received, total)`
/// fires per chunk; `on_complete(name)` fires when a file finishes and its
/// download has been triggered.
pub fn attach_receiver(
    dc: &RtcDataChannel,
    on_progress: impl Fn(String, f64, f64) + 'static,
    on_complete: impl Fn(String) + 'static,
) {
    dc.set_binary_type(web_sys::RtcDataChannelType::Arraybuffer);
    let state = Rc::new(RefCell::new(Incoming::default()));

    let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        let data = e.data();
        if let Some(text) = data.as_string() {
            // Control frame.
            match decode_control(&text) {
                Some(Control::Start(meta)) => {
                    let mut s = state.borrow_mut();
                    s.meta = Some(meta);
                    s.chunks.clear();
                    s.received = 0.0;
                }
                Some(Control::End(_)) => {
                    finalize(&state, &on_complete);
                }
                None => {}
            }
        } else {
            // Binary chunk.
            let array = js_sys::Uint8Array::new(&data);
            let mut s = state.borrow_mut();
            s.received += array.length() as f64;
            s.chunks.push(array);
            if let Some(meta) = &s.meta {
                on_progress(meta.name.clone(), s.received, meta.size);
            }
        }
    });
    dc.set_onmessage(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
}

/// Assemble received chunks into a Blob and trigger a browser download.
fn finalize(state: &Rc<RefCell<Incoming>>, on_complete: &impl Fn(String)) {
    let (meta, parts) = {
        let mut s = state.borrow_mut();
        let meta = s.meta.take();
        let parts = std::mem::take(&mut s.chunks);
        s.received = 0.0;
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
    on_complete(meta.name);
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
    let _ = Url::revoke_object_url(&url);
}

/// Drain a queue of files sequentially over the data channel.
pub fn send_files(
    dc: RtcDataChannel,
    files: Vec<File>,
    on_progress: impl Fn(String, f64, f64) + Clone + 'static,
    on_done: impl Fn() + 'static,
) {
    spawn_local(async move {
        for (i, file) in files.into_iter().enumerate() {
            let name = file.name();
            let total = file.size();
            let prog = on_progress.clone();
            let _ = send_file(dc.clone(), i as u64, file, move |sent| {
                prog(name.clone(), sent, total);
            })
            .await;
        }
        on_done();
    });
}
