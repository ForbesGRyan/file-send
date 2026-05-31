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

/// Format a byte count as a short human string (e.g. "1.5 KB").
pub fn fmt_size(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", v as u64, UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Lifecycle state of one transfer row.
#[derive(Clone, PartialEq)]
pub enum TransferState {
    /// Incoming: awaiting the local user's accept/decline. Outgoing: awaiting the peer.
    Offered,
    /// Bytes are flowing.
    Active,
    /// Finished and saved (incoming) or fully sent (outgoing).
    Done,
    /// Declined by the deciding side.
    Declined,
}

/// One transfer's UI row. Rows are keyed by `(id, incoming)`.
#[derive(Clone, PartialEq)]
pub struct Transfer {
    pub id: u64,
    pub name: String,
    pub size: f64,
    pub kind: &'static str, // type badge, e.g. "PDF"
    pub incoming: bool,     // true = receiving (peer -> me); false = sending
    pub fraction: f64,      // 0.0..=1.0
    pub state: TransferState,
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
pub fn ProgressList(
    items: ReadSignal<Vec<Transfer>>,
    on_accept: UnsyncCallback<u64>,
    on_decline: UnsyncCallback<u64>,
    on_accept_all: UnsyncCallback<()>,
) -> impl IntoView {
    // Show "Accept all" only when 2+ incoming offers are pending.
    let show_accept_all = move || {
        items.get().iter().filter(|t| t.incoming && t.state == TransferState::Offered).count() >= 2
    };
    view! {
        <Show when=show_accept_all>
            <button class="acceptall" on:click=move |_| on_accept_all.run(())>
                "Accept all"
            </button>
        </Show>
        <ul class="progress-list">
            {move || {
                items
                    .get()
                    .into_iter()
                    .map(|t| transfer_row(t, on_accept, on_decline))
                    .collect_view()
            }}
        </ul>
    }
}

/// Render one transfer row according to its state.
fn transfer_row(t: Transfer, on_accept: UnsyncCallback<u64>, on_decline: UnsyncCallback<u64>) -> impl IntoView {
    let id = t.id;
    let pct = (t.fraction * 100.0).round();
    let arrow = if t.incoming { "↓" } else { "↑" };
    match t.state {
        TransferState::Offered if t.incoming => view! {
            <li class="row offer">
                <div class="top">
                    <span>
                        <span class="diricon">{arrow}</span>" "
                        <span class="name">{t.name.clone()}</span>" "
                        <span class="tag">{t.kind}</span>" "
                        <span class="size">{fmt_size(t.size)}</span>
                    </span>
                    <span class="actions">
                        <button class="accept" on:click=move |_| on_accept.run(id)>"Accept"</button>
                        <button class="decline" on:click=move |_| on_decline.run(id)>"Decline"</button>
                    </span>
                </div>
            </li>
        }
        .into_any(),
        TransferState::Offered => view! {
            <li class="row waiting">
                <div class="top">
                    <span>
                        <span class="diricon">{arrow}</span>" "
                        <span class="name">{t.name.clone()}</span>" "
                        <span class="tag">{t.kind}</span>
                    </span>
                    <span class="pct">"WAITING…"</span>
                </div>
            </li>
        }
        .into_any(),
        TransferState::Declined => view! {
            <li class="row declined">
                <div class="top">
                    <span>
                        <span class="diricon">{arrow}</span>" "
                        <span class="name">{t.name.clone()}</span>" "
                        <span class="tag">{t.kind}</span>
                    </span>
                    <span class="pct">"✗ DECLINED"</span>
                </div>
            </li>
        }
        .into_any(),
        TransferState::Active | TransferState::Done => {
            let done = t.state == TransferState::Done;
            let row_class = if done { "row done" } else { "row" };
            let pct_label = if done { "✓ DONE".to_string() } else { format!("{pct}%") };
            let bar_style = format!("width:{pct}%");
            view! {
                <li class=row_class>
                    <div class="top">
                        <span>
                            <span class="diricon">{arrow}</span>" "
                            <span class="name">{t.name.clone()}</span>" "
                            <span class="tag">{t.kind}</span>
                        </span>
                        <span class="pct">{pct_label}</span>
                    </div>
                    <div class="bar"><i style=bar_style></i></div>
                </li>
            }
            .into_any()
        }
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

#[cfg(test)]
mod tests {
    use super::fmt_size;

    #[test]
    fn formats_human_sizes() {
        assert_eq!(fmt_size(0.0), "0 B");
        assert_eq!(fmt_size(512.0), "512 B");
        assert_eq!(fmt_size(1024.0), "1.0 KB");
        assert_eq!(fmt_size(1536.0), "1.5 KB");
        assert_eq!(fmt_size(1_048_576.0), "1.0 MB");
        assert_eq!(fmt_size(1_073_741_824.0), "1.0 GB");
    }
}
