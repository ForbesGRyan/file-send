use std::cell::RefCell;
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use shared::SignalMsg;
use wasm_bindgen::JsCast;
use web_sys::{DragEvent, HtmlInputElement, RtcDataChannel, RtcPeerConnection, RtcSdpType};

use crate::signaling::Signaling;
use crate::transfer::{attach_receiver, send_files};
use crate::ui::{FileProgress, ProgressList, Status, StatusBar};
use crate::webrtc;

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
    let (progress, set_progress) = signal(Vec::<FileProgress>::new());

    // Shared handles populated as the connection is established.
    let pc: Rc<RefCell<Option<RtcPeerConnection>>> = Rc::new(RefCell::new(None));
    let dc: Rc<RefCell<Option<RtcDataChannel>>> = Rc::new(RefCell::new(None));
    let sig: Rc<RefCell<Option<Signaling>>> = Rc::new(RefCell::new(None));

    // Helper to update one file's progress row.
    let upsert_progress = move |name: String, fraction: f64, direction: &'static str| {
        set_progress.update(|list| {
            if let Some(item) = list.iter_mut().find(|p| p.name == name && p.direction == direction) {
                item.fraction = fraction;
            } else {
                list.push(FileProgress { name, fraction, direction });
            }
        });
    };

    // Wire a freshly-available data channel: receiver + mark connected.
    let wire_dc = {
        let dc = dc.clone();
        Rc::new(move |channel: RtcDataChannel| {
            let up = upsert_progress;
            attach_receiver(
                &channel,
                move |name, recv, total| {
                    let frac = if total > 0.0 { recv / total } else { 1.0 };
                    up(name, frac, "↓ receiving");
                },
                |_name| {},
            );
            let set_status = set_status;
            let onopen = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                set_status.set(Status::Connected);
            });
            channel.set_onopen(Some(onopen.as_ref().unchecked_ref()));
            onopen.forget();
            *dc.borrow_mut() = Some(channel);
        })
    };

    // Establish signaling + peer connection on mount.
    {
        let pc = pc.clone();
        let dc_for_init = dc.clone();
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
                let _ = &dc_for_init; // channel stored inside wire_dc
            }

            // Build signaling; route inbound messages.
            let pc_msg = peer.clone();
            let sig_for_cb = sig.clone();
            let signaling = match Signaling::connect(move |msg| {
                let pc_msg = pc_msg.clone();
                let sig_for_cb = sig_for_cb.clone();
                match msg {
                    SignalMsg::Created { room } => {
                        let origin = web_sys::window().unwrap().location().origin().unwrap();
                        set_room_link.set(format!("{origin}/#/room/{room}"));
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

    // Drop-zone / file-input handler: send selected files.
    let on_files = {
        let dc = dc.clone();
        move |files: Vec<web_sys::File>| {
            if files.is_empty() {
                return;
            }
            if let Some(channel) = dc.borrow().as_ref() {
                let up = upsert_progress;
                send_files(
                    channel.clone(),
                    files,
                    move |name, sent, total| {
                        let frac = if total > 0.0 { sent / total } else { 1.0 };
                        up(name, frac, "↑ sending");
                    },
                    || {},
                );
            }
        }
    };

    let on_drop = {
        let on_files = on_files.clone();
        move |ev: DragEvent| {
            ev.prevent_default();
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

    view! {
        <main class="container">
            <h1>"file-send"</h1>
            <StatusBar status/>

            <Show when=move || !room_link.get().is_empty()>
                <div class="room-link">
                    <p>"Share this link with the other person:"</p>
                    <input
                        type="text"
                        readonly
                        prop:value=move || room_link.get()
                    />
                </div>
            </Show>

            <div
                class="drop-zone"
                on:dragover=|ev: DragEvent| ev.prevent_default()
                on:drop=on_drop
            >
                <p>"Drag & drop files here, or choose:"</p>
                <input type="file" multiple on:change=on_input_change />
            </div>

            <ProgressList items=progress/>
        </main>
    }
}
