use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::SignalMsg;
use wasm_bindgen::JsCast;
use web_sys::{DragEvent, HtmlInputElement, RtcPeerConnection, RtcSdpType};

use crate::protocol::FileStart;
use crate::qr::qr_svg;
use crate::rows;
use crate::signaling::Signaling;
use crate::transfer::{Handlers, Transfer};
use crate::ui::{JoinBox, ProgressList, ShareLink, Status, StatusBar, Transfer as Row};
use crate::webrtc;

#[cfg(test)]
mod tests {
    use super::normalize_code;

    #[test]
    fn bare_code_is_trimmed_and_lowercased() {
        assert_eq!(normalize_code("k7m4qp"), "k7m4qp");
        assert_eq!(normalize_code("  K7M4QP  "), "k7m4qp");
    }

    #[test]
    fn extracts_code_from_pasted_link_or_hash() {
        assert_eq!(
            normalize_code("http://localhost:3000/#/room/K7m4QP"),
            "k7m4qp"
        );
        assert_eq!(normalize_code("#/room/abc23"), "abc23");
        assert_eq!(
            normalize_code("https://file-send.app/#/room/k7m4qp/"),
            "k7m4qp"
        );
    }

    #[test]
    fn blank_input_yields_empty() {
        assert_eq!(normalize_code("   "), "");
    }

    use super::resolve_origin;

    #[test]
    fn resolve_origin_prefers_nonempty_override() {
        assert_eq!(
            resolve_origin(Some("https://files.example.com"), "http://localhost:3000"),
            "https://files.example.com"
        );
        // Trailing slash is stripped so links don't double up.
        assert_eq!(
            resolve_origin(Some("https://files.example.com/"), "http://localhost:3000"),
            "https://files.example.com"
        );
    }

    #[test]
    fn resolve_origin_falls_back_when_unset_or_blank() {
        assert_eq!(resolve_origin(None, "http://localhost:3000"), "http://localhost:3000");
        assert_eq!(resolve_origin(Some(""), "http://localhost:3000"), "http://localhost:3000");
        assert_eq!(resolve_origin(Some("   "), "http://localhost:3000"), "http://localhost:3000");
    }
}

/// Extract a bare room code from user input. Accepts either a raw code or a
/// pasted link/hash like `https://host/#/room/<code>`; returns the code
/// (everything after the last `room/`, stripped of surrounding slashes) trimmed
/// and lowercased, since room codes are always lowercase.
fn normalize_code(raw: &str) -> String {
    let s = raw.trim();
    let code = match s.rfind("room/") {
        Some(idx) => s[idx + "room/".len()..].trim_matches('/').trim(),
        None => s,
    };
    code.to_ascii_lowercase()
}

/// Choose the origin for share links: a non-empty compile-time override wins
/// (trailing slash stripped), otherwise the browser's runtime origin.
fn resolve_origin(compile_time: Option<&str>, runtime: &str) -> String {
    match compile_time {
        Some(o) if !o.trim().is_empty() => o.trim().trim_end_matches('/').to_string(),
        _ => runtime.to_string(),
    }
}

/// Origin used to build share links. Set `PUBLIC_ORIGIN` at build time to
/// override the browser origin — useful behind a reverse proxy or when the
/// public domain differs from what the browser sees. Otherwise the current
/// `window.location.origin` is used.
fn public_origin() -> String {
    let runtime = web_sys::window().unwrap().location().origin().unwrap();
    resolve_origin(option_env!("PUBLIC_ORIGIN"), &runtime)
}

/// Read the room id from the URL hash (`#/room/<id>`), if present.
fn room_from_hash() -> Option<String> {
    let hash = web_sys::window().unwrap().location().hash().ok()?;
    let trimmed = hash.trim_start_matches('#').trim_start_matches('/');
    let rest = trimmed.strip_prefix("room/")?;
    if rest.is_empty() { None } else { Some(rest.to_string()) }
}

/// Key under which a tab records the room it created (so a refresh can reclaim it).
const OWNS_KEY: &str = "file-send:owns";

/// The browser's per-tab session storage, if available (absent in some privacy
/// modes). Session storage — not local storage — because ownership is scoped to
/// this tab and should not bleed into other tabs or survive the tab closing.
fn session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

/// The room id this tab created, remembered across refreshes.
fn session_owns() -> Option<String> {
    session_storage()?.get_item(OWNS_KEY).ok().flatten()
}

/// Remember that this tab owns `room`, so a refresh reclaims it as the initiator.
fn set_session_owns(room: &str) {
    if let Some(s) = session_storage() {
        let _ = s.set_item(OWNS_KEY, room);
    }
}

#[component]
pub fn App() -> impl IntoView {
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
    // Monotonic id for outgoing files. Assigned the moment a file is added (not when
    // its offer is sent) so a file picked before the channel is open can be shown as
    // a Pending row right away and later reconciled by id.
    let next_id: Rc<Cell<u64>> = Rc::new(Cell::new(0));
    // Files chosen before the channel is open, with their assigned ids; their offers
    // are sent on open.
    let pending: Rc<RefCell<Vec<(u64, web_sys::File)>>> = Rc::new(RefCell::new(Vec::new()));

    // Build the transfer event handlers. Each is a thin wrapper that applies a
    // pure `rows` transition inside `set_items.update`; the logic itself lives
    // in the `rows` module so it can be tested without a browser runtime.
    let handlers = Handlers {
        on_offer: Rc::new(move |meta: FileStart| {
            set_items.update(|list| rows::incoming_offer(list, &meta));
        }),
        on_recv_progress: Rc::new(move |id, name, recv, total, speed| {
            set_items.update(|list| rows::recv_progress(list, id, &name, recv, total, speed));
        }),
        on_recv_complete: Rc::new(move |id, name| {
            set_items.update(|list| rows::recv_complete(list, id, &name));
        }),
        on_send_progress: Rc::new(move |id, name, sent, total| {
            set_items.update(|list| rows::send_progress(list, id, &name, sent, total));
        }),
        on_rejected: Rc::new(move |id| {
            set_items.update(|list| rows::mark_rejected(list, id));
        }),
        on_cancelled: Rc::new(move |id| {
            set_items.update(|list| rows::mark_cancelled_remote(list, id));
        }),
    };

    // Send offers for a batch of already-id'd files (their Pending rows already
    // exist) and flip each row Pending -> Offered.
    let offer_now: Rc<dyn Fn(Vec<(u64, web_sys::File)>)> = {
        let transfer = transfer.clone();
        Rc::new(move |files: Vec<(u64, web_sys::File)>| {
            let ids: Vec<u64> = files.iter().map(|(id, _)| *id).collect();
            if let Some(t) = transfer.borrow().as_ref() {
                t.offer_files(files);
            }
            for id in ids {
                set_items.update(|list| rows::mark_offered(list, id));
            }
        })
    };

    // Wire the control channel: build the Transfer + flush the queue on open.
    let wire_ctrl: Rc<dyn Fn(RtcPeerConnection, web_sys::RtcDataChannel)> = {
        let transfer = transfer.clone();
        let pending = pending.clone();
        let offer_now = offer_now.clone();
        let handlers = handlers.clone();
        Rc::new(move |peer: RtcPeerConnection, channel: web_sys::RtcDataChannel| {
            let t = Transfer::new(peer, channel, handlers.clone());
            // On open: mark connected and offer any files queued before connect.
            let pending = pending.clone();
            let offer_now = offer_now.clone();
            let onopen = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                set_status.set(Status::Connected);
                let queued: Vec<(u64, web_sys::File)> = pending.borrow_mut().drain(..).collect();
                if !queued.is_empty() {
                    offer_now(queued);
                }
            });
            t.channel_set_onopen(onopen);
            *transfer.borrow_mut() = Some(t);
        })
    };

    // Establish signaling + peer connection on mount.
    {
        let pc = pc.clone();
        let sig = sig.clone();
        let wire_ctrl = wire_ctrl.clone();
        let transfer = transfer.clone();
        Effect::new(move |_| {
            // Role for this load. A tab that created a room remembers its id in
            // sessionStorage (which survives a refresh but not a fresh tab), so on
            // reload the owner *reclaims* the same room instead of trying to join one
            // the server already dropped. Both creating and reclaiming make us the
            // initiator; only joining someone else's link makes us the joiner.
            let hash_room = room_from_hash();
            let reclaim = match (&hash_room, &session_owns()) {
                (Some(h), Some(owned)) => h == owned,
                _ => false,
            };
            let is_initiator = hash_room.is_none() || reclaim;
            set_status.set(Status::Connecting);

            let peer = match webrtc::new_peer_connection() {
                Ok(p) => p,
                Err(e) => {
                    set_status.set(Status::Error(format!("{e:?}")));
                    return;
                }
            };
            *pc.borrow_mut() = Some(peer.clone());

            // Both peers listen for inbound channels: the control channel (joiner
            // side) and per-file channels (whichever side is receiving a file).
            // Channels are routed by label — `CTRL_LABEL` vs a numeric file id.
            {
                let wire = wire_ctrl.clone();
                let transfer = transfer.clone();
                let peer_for_dc = peer.clone();
                webrtc::on_data_channel(&peer, move |ch| {
                    if ch.label() == crate::transfer::CTRL_LABEL {
                        wire(peer_for_dc.clone(), ch);
                    } else if let Some(t) = transfer.borrow().as_ref() {
                        t.handle_incoming_channel(ch);
                    }
                });
            }
            // Initiator (creator or reclaiming owner) creates the control channel up
            // front; the joiner receives it via `ondatachannel` above.
            if is_initiator {
                let channel = webrtc::create_data_channel(&peer, crate::transfer::CTRL_LABEL);
                wire_ctrl(peer.clone(), channel);
            }

            // Build signaling; route inbound messages.
            let pc_msg = peer.clone();
            let sig_for_cb = sig.clone();
            let signaling = match Signaling::connect(move |msg| {
                let pc_msg = pc_msg.clone();
                let sig_for_cb = sig_for_cb.clone();
                match msg {
                    SignalMsg::Created { room } => {
                        let link = format!("{}/#/room/{room}", public_origin());
                        set_qr.set(qr_svg(&link));
                        set_room_link.set(link);
                        // Persist the room in the URL hash so a refresh rejoins the
                        // same room instead of silently creating a brand-new one, and
                        // remember (per tab) that we own it so the refresh reclaims it
                        // rather than failing to join a torn-down room.
                        let _ = web_sys::window()
                            .unwrap()
                            .location()
                            .set_hash(&format!("/room/{room}"));
                        set_session_owns(&room);
                        set_room_code.set(room);
                        set_status.set(Status::WaitingForPeer);
                    }
                    SignalMsg::PeerJoined => {
                        // Initiator: create and send the offer.
                        spawn_local(async move {
                            if let Ok(sdp) = webrtc::create_offer(&pc_msg).await {
                                if let Some(s) = sig_for_cb.borrow().as_ref() {
                                    s.send(&SignalMsg::Sdp { sdp, kind: "offer".into() });
                                }
                            }
                        });
                    }
                    SignalMsg::Sdp { sdp, kind } => {
                        spawn_local(async move {
                            if kind == "offer" {
                                let _ = webrtc::set_remote(&pc_msg, RtcSdpType::Offer, &sdp).await;
                                if let Ok(answer) = webrtc::create_answer(&pc_msg).await {
                                    if let Some(s) = sig_for_cb.borrow().as_ref() {
                                        s.send(&SignalMsg::Sdp { sdp: answer, kind: "answer".into() });
                                    }
                                }
                            } else {
                                let _ = webrtc::set_remote(&pc_msg, RtcSdpType::Answer, &sdp).await;
                            }
                        });
                    }
                    SignalMsg::Ice { candidate } => {
                        spawn_local(async move {
                            let _ = webrtc::add_ice_candidate(&pc_msg, &candidate).await;
                        });
                    }
                    SignalMsg::PeerLeft => set_status.set(Status::PeerLeft),
                    SignalMsg::RoomFull => set_status.set(Status::RoomFull),
                    SignalMsg::RoomNotFound => set_status.set(Status::RoomNotFound),
                    _ => {}
                }
            }) {
                Ok(s) => s,
                Err(e) => {
                    set_status.set(Status::Error(format!("{e:?}")));
                    return;
                }
            };

            // Forward local ICE candidates out through signaling.
            let sig_ice = sig.clone();
            webrtc::on_ice_candidate(&peer, move |candidate| {
                if let Some(s) = sig_ice.borrow().as_ref() {
                    s.send(&SignalMsg::Ice { candidate });
                }
            });

            // On open: create a fresh room, reclaim our own room after a refresh, or
            // join someone else's by id.
            let sig_open = signaling.clone();
            signaling.on_open(move || match room_from_hash() {
                None => sig_open.send(&SignalMsg::Create),
                Some(room) if reclaim => sig_open.send(&SignalMsg::Reclaim { room }),
                Some(room) => sig_open.send(&SignalMsg::Join { room }),
            });

            *sig.borrow_mut() = Some(signaling);
        });
    }

    // File-input/drop handler: show a row for every file immediately, then offer
    // now if the channel is open, else queue the offer for on-open. Assigning the
    // id and rendering the row up front means a file added before the connection is
    // ready appears as Pending instead of silently disappearing.
    let on_files = {
        let transfer = transfer.clone();
        let pending = pending.clone();
        let offer_now = offer_now.clone();
        let next_id = next_id.clone();
        move |files: Vec<web_sys::File>| {
            if files.is_empty() {
                return;
            }
            let mut batch = Vec::with_capacity(files.len());
            for file in files {
                let id = next_id.get();
                next_id.set(id + 1);
                set_items
                    .update(|list| rows::push_outgoing_pending(list, id, &file.name(), file.size()));
                batch.push((id, file));
            }
            let open = transfer.borrow().as_ref().map(|t| t.is_open()).unwrap_or(false);
            if open {
                offer_now(batch);
            } else {
                pending.borrow_mut().extend(batch);
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
            set_items.update(|list| rows::accept(list, id));
        }
    };
    let on_decline = {
        let transfer = transfer.clone();
        move |id: u64| {
            if let Some(t) = transfer.borrow().as_ref() {
                t.reject(id);
            }
            // Remove the declined incoming row locally.
            set_items.update(|list| rows::decline(list, id));
        }
    };
    // Cancel an in-progress incoming download: stop the sender, mark the row.
    let on_cancel = {
        let transfer = transfer.clone();
        move |id: u64| {
            if let Some(t) = transfer.borrow().as_ref() {
                t.cancel(id);
            }
            set_items.update(|list| rows::cancel(list, id));
        }
    };
    let on_accept_all = {
        let transfer = transfer.clone();
        move || {
            let ids = rows::pending_incoming_ids(&items.get_untracked());
            if let Some(t) = transfer.borrow().as_ref() {
                for id in &ids {
                    t.accept(*id);
                }
            }
            set_items.update(|list| rows::accept_all(list, &ids));
        }
    };

    let on_drop = {
        let on_files = on_files.clone();
        move |ev: DragEvent| {
            ev.prevent_default();
            set_drag_depth.set(0);
            let mut files = Vec::new();
            if let Some(dt) = ev.data_transfer() {
                if let Some(list) = dt.files() {
                    for i in 0..list.length() {
                        if let Some(f) = list.item(i) {
                            files.push(f);
                        }
                    }
                }
            }
            on_files(files);
        }
    };

    let on_input_change = {
        let on_files = on_files.clone();
        move |ev: leptos::ev::Event| {
            let input: HtmlInputElement = ev.target().unwrap().unchecked_into();
            let mut files = Vec::new();
            if let Some(list) = input.files() {
                for i in 0..list.length() {
                    if let Some(f) = list.item(i) {
                        files.push(f);
                    }
                }
            }
            on_files(files);
        }
    };

    let drop_class = move || if drag_depth.get() > 0 { "drop active" } else { "drop" };

    // Join an existing room by code: set the hash and reload so the mount
    // effect re-runs as a joiner. A bad code surfaces as RoomNotFound.
    let on_join = Callback::new(move |raw: String| {
        let code = normalize_code(&raw);
        if code.is_empty() {
            return;
        }
        let loc = web_sys::window().unwrap().location();
        let _ = loc.set_hash(&format!("/room/{code}"));
        let _ = loc.reload();
    });

    view! {
        <main class="container">
            <h1 class="wm">"File"<span>"·"</span><br/>"Send"</h1>
            <p class="tagline">"Peer-to-peer // bytes never touch a server"</p>

            <StatusBar status/>

            // Dead-end recovery: a missing/expired room leaves nowhere to go, so
            // offer a one-click escape to the root, which starts a fresh room.
            <Show when=move || status.get() == Status::RoomNotFound>
                <button
                    class="newroom"
                    on:click=move |_| {
                        let _ = web_sys::window().unwrap().location().set_href("/");
                    }
                >
                    "Start a new room"
                </button>
            </Show>

            <Show when=move || !room_link.get().is_empty()>
                <ShareLink code=room_code link=room_link qr=qr/>
            </Show>

            <Show when=move || status.get() != Status::Connected>
                <JoinBox on_join=on_join/>
            </Show>

            <div
                class=drop_class
                on:dragenter=move |ev: DragEvent| { ev.prevent_default(); set_drag_depth.update(|d| *d += 1); }
                on:dragover=move |ev: DragEvent| ev.prevent_default()
                on:dragleave=move |_| set_drag_depth.update(|d| if *d > 0 { *d -= 1; })
                on:drop=on_drop
            >
                <b>"Drop files here"</b>
                <span class="sub">"— or —"</span>
                <label class="filebtn">
                    "Select files"
                    <input type="file" multiple on:change=on_input_change />
                </label>
            </div>

            <ProgressList
                items=items
                on_accept=UnsyncCallback::new(on_accept)
                on_decline=UnsyncCallback::new(on_decline)
                on_cancel=UnsyncCallback::new(on_cancel)
                on_accept_all=UnsyncCallback::new(move |_| on_accept_all())
            />
        </main>
    }
}
