//! Self-reconnecting signaling WebSocket client.

use std::cell::{Cell, RefCell};
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

/// Live JS closures for one socket. Held so they outlive the socket and are
/// dropped (freeing the old socket's handlers) when a reconnect installs a new
/// socket's closures.
struct SocketCallbacks {
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onopen: Closure<dyn FnMut()>,
    _onclose: Closure<dyn FnMut()>,
}

/// Shared, reference-counted signaling state. The `online` listener and the
/// per-socket closures hold clones of this `Rc`, forming a deliberate cycle:
/// `Signaling` is a page-lifetime singleton, so it is never meant to be dropped
/// (mirrors the original `cb.forget()` pattern).
struct SignalingInner {
    url: String,
    ws: RefCell<WebSocket>,
    on_msg: Rc<dyn Fn(SignalMsg)>,
    on_open: RefCell<Option<Rc<dyn Fn()>>>,
    on_disconnect: RefCell<Option<Rc<dyn Fn()>>>,
    /// Consecutive failed-reconnect counter; reset to 0 on a successful open.
    attempt: Cell<u32>,
    /// Set before an intentional close so `onclose` does not reconnect.
    closed: Cell<bool>,
    callbacks: RefCell<Option<SocketCallbacks>>,
    _online: RefCell<Option<Closure<dyn FnMut()>>>,
}

/// Owns a browser WebSocket and transparently reconnects it (capped exponential
/// backoff, plus an immediate retry on the browser `online` event). Routes
/// inbound `SignalMsg`s to `on_msg`; fires `on_open` on every (re)open and
/// `on_disconnect` on every unexpected drop.
#[derive(Clone)]
pub struct Signaling {
    inner: Rc<SignalingInner>,
}

impl Signaling {
    /// Connect to the signaling endpoint (relative `/ws` on the current host).
    /// `on_msg` is called for every successfully parsed inbound message, across
    /// reconnects.
    pub fn connect(on_msg: impl Fn(SignalMsg) + 'static) -> Result<Self, JsValue> {
        let location = web_sys::window().unwrap().location();
        let proto = if location.protocol()? == "https:" { "wss" } else { "ws" };
        let host = location.host()?;
        let url = format!("{proto}://{host}/ws");

        let ws = WebSocket::new(&url)?;
        let inner = Rc::new(SignalingInner {
            url,
            ws: RefCell::new(ws),
            on_msg: Rc::new(on_msg),
            on_open: RefCell::new(None),
            on_disconnect: RefCell::new(None),
            attempt: Cell::new(0),
            closed: Cell::new(false),
            callbacks: RefCell::new(None),
            _online: RefCell::new(None),
        });
        wire_socket(&inner);
        register_online(&inner);
        Ok(Self { inner })
    }

    /// Send a signaling message on the current socket (no-op if not open yet).
    pub fn send(&self, msg: &SignalMsg) {
        if let Ok(json) = serde_json::to_string(msg) {
            let _ = self.inner.ws.borrow().send_with_str(&json);
        }
    }

    /// Register a callback fired on every (re)connect once the socket opens.
    pub fn on_open(&self, f: impl Fn() + 'static) {
        *self.inner.on_open.borrow_mut() = Some(Rc::new(f));
    }

    /// Register a callback fired whenever the socket drops unexpectedly (before
    /// the reconnect is scheduled).
    pub fn on_disconnect(&self, f: impl Fn() + 'static) {
        *self.inner.on_disconnect.borrow_mut() = Some(Rc::new(f));
    }
}

/// Wire `onmessage`/`onopen`/`onclose` onto the inner's current socket and store
/// the closures (dropping any previous socket's closures).
fn wire_socket(inner: &Rc<SignalingInner>) {
    let ws = inner.ws.borrow().clone();

    let onmessage = {
        let on_msg = inner.on_msg.clone();
        Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            if let Some(text) = e.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<SignalMsg>(&text) {
                    on_msg(msg);
                }
            }
        })
    };
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

    let onopen = {
        let inner = inner.clone();
        Closure::<dyn FnMut()>::new(move || {
            inner.attempt.set(0);
            if let Some(f) = inner.on_open.borrow().as_ref() {
                f();
            }
        })
    };
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));

    let onclose = {
        let inner = inner.clone();
        Closure::<dyn FnMut()>::new(move || {
            if inner.closed.get() {
                return;
            }
            crate::log::clog("[ws] closed -> reconnecting");
            if let Some(f) = inner.on_disconnect.borrow().as_ref() {
                f();
            }
            schedule_reconnect(&inner);
        })
    };
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

    *inner.callbacks.borrow_mut() = Some(SocketCallbacks {
        _onmessage: onmessage,
        _onopen: onopen,
        _onclose: onclose,
    });
}

/// Schedule a reconnect after `next_backoff(attempt)` ms, incrementing the
/// attempt counter. Runs out of the `onclose` call stack via `set_timeout`.
fn schedule_reconnect(inner: &Rc<SignalingInner>) {
    let delay = next_backoff(inner.attempt.get()) as i32;
    inner.attempt.set(inner.attempt.get() + 1);
    let inner2 = inner.clone();
    let cb = Closure::once_into_js(move || reconnect_now(&inner2));
    let _ = web_sys::window()
        .unwrap()
        .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), delay);
}

/// Build a fresh socket and wire it, unless intentionally closed or a connect is
/// already in flight/open (so a backoff tick and an `online` event can't double-
/// connect). On construction failure, retry on the next backoff tick.
fn reconnect_now(inner: &Rc<SignalingInner>) {
    if inner.closed.get() {
        return;
    }
    let state = inner.ws.borrow().ready_state();
    if state == WebSocket::OPEN || state == WebSocket::CONNECTING {
        return;
    }
    match WebSocket::new(&inner.url) {
        Ok(ws) => {
            *inner.ws.borrow_mut() = ws;
            wire_socket(inner);
        }
        Err(_) => schedule_reconnect(inner),
    }
}

/// Reconnect immediately when the browser regains connectivity (e.g. wake from
/// sleep), resetting the backoff — but only if the current socket isn't already
/// open/connecting.
fn register_online(inner: &Rc<SignalingInner>) {
    let inner2 = inner.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        if inner2.closed.get() {
            return;
        }
        let state = inner2.ws.borrow().ready_state();
        if state == WebSocket::OPEN || state == WebSocket::CONNECTING {
            return;
        }
        crate::log::clog("[ws] online -> reconnecting now");
        inner2.attempt.set(0);
        reconnect_now(&inner2);
    });
    let _ = web_sys::window()
        .unwrap()
        .add_event_listener_with_callback("online", cb.as_ref().unchecked_ref());
    *inner._online.borrow_mut() = Some(cb);
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
