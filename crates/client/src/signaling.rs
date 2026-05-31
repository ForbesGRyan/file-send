//! Signaling WebSocket client wrapper.

use std::rc::Rc;

use shared::SignalMsg;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

/// Owns a browser WebSocket and routes inbound `SignalMsg`s to a callback.
#[derive(Clone)]
pub struct Signaling {
    ws: WebSocket,
}

impl Signaling {
    /// Connect to the signaling endpoint (relative `/ws` on the current host).
    /// `on_msg` is called for every successfully parsed inbound message.
    pub fn connect(on_msg: impl Fn(SignalMsg) + 'static) -> Result<Self, JsValue> {
        let location = web_sys::window().unwrap().location();
        let proto = if location.protocol()? == "https:" { "wss" } else { "ws" };
        let host = location.host()?;
        let url = format!("{proto}://{host}/ws");

        let ws = WebSocket::new(&url)?;

        let on_msg = Rc::new(on_msg);
        let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            if let Some(text) = e.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<SignalMsg>(&text) {
                    on_msg(msg);
                }
            }
        });
        ws.set_onmessage(Some(cb.as_ref().unchecked_ref()));
        cb.forget(); // keep the closure alive for the socket's lifetime

        Ok(Self { ws })
    }

    /// Send a signaling message (no-op if the socket is not open yet — callers
    /// should send after the `open` event; see `on_open`).
    pub fn send(&self, msg: &SignalMsg) {
        if let Ok(json) = serde_json::to_string(msg) {
            let _ = self.ws.send_with_str(&json);
        }
    }

    /// Register a callback fired once the socket opens.
    pub fn on_open(&self, f: impl Fn() + 'static) {
        let cb = Closure::<dyn FnMut()>::new(move || f());
        self.ws.set_onopen(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    }
}
