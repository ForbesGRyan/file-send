//! Leptos UI components.

use leptos::prelude::*;

/// Connection status shown to the user.
#[derive(Clone, PartialEq)]
pub enum Status {
    Idle,
    WaitingForPeer,
    Connecting,
    Connected,
    PeerLeft,
    RoomFull,
    RoomNotFound,
    Error(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Idle => "Idle".into(),
            Status::WaitingForPeer => "Waiting for peer to join…".into(),
            Status::Connecting => "Connecting…".into(),
            Status::Connected => "Connected — ready to transfer".into(),
            Status::PeerLeft => "Peer disconnected".into(),
            Status::RoomFull => "Room is full".into(),
            Status::RoomNotFound => "Room not found or expired".into(),
            Status::Error(e) => format!("Couldn't establish direct connection: {e}"),
        }
    }
}

/// One file's transfer progress (0.0..=1.0).
#[derive(Clone, PartialEq)]
pub struct FileProgress {
    pub name: String,
    pub fraction: f64,
    pub direction: &'static str, // "↑" sending / "↓" receiving
    pub kind: &'static str,      // type badge, e.g. "PDF", "IMG"
    pub done: bool,
}

#[component]
pub fn StatusBar(status: ReadSignal<Status>) -> impl IntoView {
    view! {
        <div class="statusrow">
            <span class="dot"></span>
            <span class="status">{move || status.get().label()}</span>
        </div>
    }
}

#[component]
pub fn ProgressList(items: ReadSignal<Vec<FileProgress>>) -> impl IntoView {
    view! {
        <ul class="progress-list">
            {move || {
                items
                    .get()
                    .into_iter()
                    .map(|p| {
                        let pct = (p.fraction * 100.0).round();
                        let row_class = if p.done { "row done" } else { "row" };
                        let pct_label = if p.done {
                            "✓ DONE".to_string()
                        } else {
                            format!("{pct}%")
                        };
                        let bar_style = format!("width:{pct}%");
                        view! {
                            <li class=row_class>
                                <div class="top">
                                    <span>
                                        <span class="diricon">{p.direction}</span>
                                        " "
                                        <span class="name">{p.name.clone()}</span>
                                        " "
                                        <span class="tag">{p.kind}</span>
                                    </span>
                                    <span class="pct">{pct_label}</span>
                                </div>
                                <div class="bar"><i style=bar_style></i></div>
                            </li>
                        }
                    })
                    .collect_view()
            }}
        </ul>
    }
}

/// Share block: readonly room link with a copy button, plus a QR code
/// (an SVG string injected as inner HTML) for phone scanning.
#[component]
pub fn ShareLink(
    link: ReadSignal<String>,
    qr: ReadSignal<String>,
) -> impl IntoView {
    let copy = move |_| {
        let text = link.get_untracked();
        if let Some(win) = web_sys::window() {
            // Fire-and-forget; the returned Promise is intentionally ignored.
            let _ = win.navigator().clipboard().write_text(&text);
        }
    };
    view! {
        <div class="block share">
            <div class="left">
                <p class="label">"Share to connect a peer"</p>
                <div class="linkrow">
                    <input type="text" readonly prop:value=move || link.get() />
                    <button class="copybtn" on:click=copy>"Copy"</button>
                </div>
                <p class="note">"Link expires when you close the tab."</p>
            </div>
            <div class="qrwrap">
                <div class="qr" inner_html=move || qr.get()></div>
                <p class="qr-cap">"scan on phone"</p>
            </div>
        </div>
    }
}
