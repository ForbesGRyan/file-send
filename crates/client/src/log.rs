//! Tiny `console.log` helpers for the sleep/reconnect investigation.
//!
//! Temporary diagnostic instrumentation: the signaling/WebRTC handshake
//! swallows JS errors (`let _ = ...`, `if let Ok(...)`), so a failing
//! `set_remote`/`create_answer` is silent. These log each handshake step and the
//! actual error value to the browser console. Remove once the root cause is
//! fixed. View on Android via `chrome://inspect` (USB remote debugging).

use wasm_bindgen::JsValue;

/// Log a message to the browser console.
pub fn clog(msg: &str) {
    web_sys::console::log_1(&JsValue::from_str(msg));
}

/// Log a message alongside a JS value (e.g. an error from a rejected promise).
pub fn clog_val(msg: &str, val: &JsValue) {
    web_sys::console::log_2(&JsValue::from_str(msg), val);
}
