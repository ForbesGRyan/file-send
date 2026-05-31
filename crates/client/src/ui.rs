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

/// Copy `text` to the clipboard, fire-and-forget (the returned Promise is
/// intentionally ignored).
fn copy_to_clipboard(text: &str) {
    if let Some(win) = web_sys::window() {
        let _ = win.navigator().clipboard().write_text(text);
    }
}

/// Share block: a prominent room code with a copy button and a QR code for
/// phone scanning, plus the full share link demoted to a secondary row.
#[component]
pub fn ShareLink(
    code: ReadSignal<String>,
    link: ReadSignal<String>,
    qr: ReadSignal<String>,
) -> impl IntoView {
    let copy_code = move |_| copy_to_clipboard(&code.get_untracked());
    let copy_link = move |_| copy_to_clipboard(&link.get_untracked());
    view! {
        <div class="block share">
            <div class="left">
                <p class="label">"Share to connect a peer"</p>
                <p class="codelabel">"Your room code"</p>
                <div class="coderow">
                    <span class="code">{move || code.get()}</span>
                    <button class="copycode" on:click=copy_code>"Copy code"</button>
                </div>
                <div class="sharelink">
                    <span class="sharelink-cap">"or link:"</span>
                    <input type="text" readonly prop:value=move || link.get() />
                    <button class="copybtn small" on:click=copy_link>"Copy"</button>
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

/// Input box to join an existing room by typing or pasting a code (or a link).
#[component]
pub fn JoinBox(on_join: Callback<String>) -> impl IntoView {
    let (value, set_value) = signal(String::new());
    let submit = move || on_join.run(value.get_untracked());
    view! {
        <div class="block joinbox">
            <p class="label">"Have a code? Join a room"</p>
            <div class="joinrow">
                <input
                    type="text"
                    class="joininput"
                    placeholder="enter code or paste link"
                    prop:value=move || value.get()
                    on:input=move |ev| set_value.set(event_target_value(&ev))
                    on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                        if ev.key() == "Enter" {
                            submit();
                        }
                    }
                />
                <button class="joinbtn" on:click=move |_| submit()>"Join"</button>
            </div>
        </div>
    }
}
