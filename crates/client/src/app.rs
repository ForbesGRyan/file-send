use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::SignalMsg;
use wasm_bindgen::JsCast;
use web_sys::{DragEvent, HtmlInputElement, RtcPeerConnection, RtcSdpType};

use crate::filetype::file_kind;
use crate::protocol::FileStart;
use crate::qr::qr_svg;
use crate::signaling::Signaling;
use crate::transfer::{Handlers, Transfer};
use crate::ui::{JoinBox, ProgressList, ShareLink, Status, StatusBar, Transfer as Row, TransferState};
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
    // Files chosen before the channel is open; their offers are sent on open.
    let pending: Rc<RefCell<Vec<web_sys::File>>> = Rc::new(RefCell::new(Vec::new()));

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

    // Establish signaling + peer connection on mount.
    {
        let pc = pc.clone();
        let sig = sig.clone();
        let wire_dc = wire_dc.clone();
        Effect::new(move |_| {
            let is_joiner = room_from_hash().is_some();
            set_status.set(Status::Connecting);

            let peer = match webrtc::new_peer_connection() {
                Ok(p) => p,
                Err(e) => {
                    set_status.set(Status::Error(format!("{e:?}")));
                    return;
                }
            };
            *pc.borrow_mut() = Some(peer.clone());

            // Joiner waits for the initiator-created channel.
            if is_joiner {
                let wire = wire_dc.clone();
                webrtc::on_data_channel(&peer, move |ch| wire(ch));
            } else {
                // Initiator creates the channel up front.
                let channel = webrtc::create_data_channel(&peer);
                wire_dc(channel);
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

            // On open, either create the room or join it.
            let sig_open = signaling.clone();
            signaling.on_open(move || {
                match room_from_hash() {
                    Some(room) => sig_open.send(&SignalMsg::Join { room }),
                    None => sig_open.send(&SignalMsg::Create),
                }
            });

            *sig.borrow_mut() = Some(signaling);
        });
    }

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
                <span class="sub">"— or click to choose —"</span>
                <input type="file" multiple on:change=on_input_change />
            </div>

            <ProgressList
                items=items
                on_accept=UnsyncCallback::new(on_accept)
                on_decline=UnsyncCallback::new(on_decline)
                on_accept_all=UnsyncCallback::new(move |_| on_accept_all())
            />
        </main>
    }
}
