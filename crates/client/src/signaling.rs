//! Signaling WebSocket client wrapper.

use std::rc::Rc;

use shared::SignalMsg;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

/// Base reconnect delay in ms; doubles each attempt up to `BACKOFF_CAP_MS`.
const BACKOFF_BASE_MS: u32 = 500;
/// Maximum reconnect delay in ms.
const BACKOFF_CAP_MS: u32 = 15_000;

/// Reconnect delay for the Nth consecutive attempt (0-based): exponential
/// backoff `500 * 2^attempt`, capped at 15s. Pure, for unit testing and so the
/// reconnect timer and any future caller share one definition. Uses `u64`
/// internally so a large `attempt` saturates to the cap instead of overflowing.
pub fn next_backoff(attempt: u32) -> u32 {
    let shifted = (BACKOFF_BASE_MS as u64).checked_shl(attempt).unwrap_or(u64::MAX);
    shifted.min(BACKOFF_CAP_MS as u64) as u32
}

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

#[cfg(test)]
mod tests {
    use super::next_backoff;

    #[test]
    fn backoff_ramps_then_caps() {
        // 500 * 2^attempt, capped at 15_000 ms.
        assert_eq!(next_backoff(0), 500);
        assert_eq!(next_backoff(1), 1_000);
        assert_eq!(next_backoff(2), 2_000);
        assert_eq!(next_backoff(3), 4_000);
        assert_eq!(next_backoff(4), 8_000);
        // 500 * 2^5 = 16_000 -> capped.
        assert_eq!(next_backoff(5), 15_000);
        assert_eq!(next_backoff(6), 15_000);
        // Large attempts never overflow or drop below the cap.
        assert_eq!(next_backoff(40), 15_000);
        assert_eq!(next_backoff(100), 15_000);
    }
}
