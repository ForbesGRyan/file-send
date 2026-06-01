//! Leptos UI components.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TransferState {
    /// Outgoing only: added locally but not yet offered because the channel isn't
    /// open yet (e.g. still waiting for a peer to join). Flips to `Offered` once the
    /// offer is sent.
    Pending,
    /// Incoming: awaiting the local user's accept/decline. Outgoing: awaiting the peer.
    Offered,
    /// Bytes are flowing.
    Active,
    /// Finished and saved (incoming) or fully sent (outgoing).
    Done,
    /// Declined by the deciding side.
    Declined,
    /// Aborted mid-transfer (receiver cancelled).
    Cancelled,
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
    pub speed: f64,         // bytes/sec, receive-side estimate (0.0 = unknown)
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
    on_cancel: UnsyncCallback<u64>,
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
            <For
                // Key on identity + lifecycle state, NOT fraction/speed. While a
                // download is Active the key stays constant, so <For> keeps the
                // row's DOM (and its Cancel button) alive across the hundreds of
                // progress ticks instead of rebuilding the list each time. A button
                // recreated mid-press eats the click — which is why Cancel was dead.
                each=move || items.get()
                key=|t| (t.id, t.incoming, t.state.clone())
                children=move |t| transfer_row(t, items, on_accept, on_decline, on_cancel)
            />
        </ul>
    }
}

/// Format a transfer rate (bytes/sec) as a short string, e.g. "1.5 MB/s".
fn fmt_speed(bytes_per_sec: f64) -> String {
    format!("{}/s", fmt_size(bytes_per_sec))
}

/// Render one transfer row according to its state.
fn transfer_row(
    t: Transfer,
    items: ReadSignal<Vec<Transfer>>,
    on_accept: UnsyncCallback<u64>,
    on_decline: UnsyncCallback<u64>,
    on_cancel: UnsyncCallback<u64>,
) -> impl IntoView {
    let id = t.id;
    let incoming = t.incoming;
    let arrow = if t.incoming { "↓" } else { "↑" };
    match t.state {
        // Added but not yet offered (channel not open): show it waiting instead of
        // letting the file vanish into the pre-connection queue.
        TransferState::Pending => view! {
            <li class="row waiting">
                <div class="top">
                    <span>
                        <span class="diricon">{arrow}</span>" "
                        <span class="name">{t.name.clone()}</span>" "
                        <span class="tag">{t.kind}</span>
                    </span>
                    <span class="pct">"PENDING…"</span>
                </div>
            </li>
        }
        .into_any(),
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
        TransferState::Declined | TransferState::Cancelled => {
            let label = if t.state == TransferState::Cancelled { "✗ CANCELLED" } else { "✗ DECLINED" };
            view! {
                <li class="row declined">
                    <div class="top">
                        <span>
                            <span class="diricon">{arrow}</span>" "
                            <span class="name">{t.name.clone()}</span>" "
                            <span class="tag">{t.kind}</span>
                        </span>
                        <span class="pct">{label}</span>
                    </div>
                </li>
            }
            .into_any()
        }
        TransferState::Active | TransferState::Done => {
            let done = t.state == TransferState::Done;
            let row_class = if done { "row done" } else { "row" };
            // This row's lifecycle state is fixed for its <For> key, so only the
            // numbers move. Re-read fraction/speed live from the list on each tick
            // via reactive closures: they update the bar/pct/speed in place without
            // tearing down the row, keeping the Cancel button's DOM node stable.
            let frac = move || {
                items.with(|l| {
                    l.iter()
                        .find(|r| r.id == id && r.incoming == incoming)
                        .map(|r| r.fraction)
                        .unwrap_or(if done { 1.0 } else { 0.0 })
                })
            };
            let speed = move || {
                items.with(|l| {
                    l.iter()
                        .find(|r| r.id == id && r.incoming == incoming)
                        .map(|r| r.speed)
                        .unwrap_or(0.0)
                })
            };
            let pct_label =
                move || if done { "✓ DONE".to_string() } else { format!("{}%", (frac() * 100.0).round()) };
            let bar_style = move || format!("width:{}%", (frac() * 100.0).round());
            // Live transfer rate, shown only while an incoming file is flowing.
            let show_speed = move || incoming && !done && speed() > 0.0;
            let speed_label = move || fmt_speed(speed());
            // Cancel is offered only on an in-progress download (fixed per key).
            let show_cancel = incoming && !done;
            view! {
                <li class=row_class>
                    <div class="top">
                        <span>
                            <span class="diricon">{arrow}</span>" "
                            <span class="name">{t.name.clone()}</span>" "
                            <span class="tag">{t.kind}</span>
                        </span>
                        <span class="meta">
                            <Show when=show_speed>
                                <span class="speed">{speed_label}</span>
                            </Show>
                            <span class="pct">{pct_label}</span>
                            <Show when=move || show_cancel>
                                <button class="cancel" on:click=move |_| on_cancel.run(id)>"Cancel"</button>
                            </Show>
                        </span>
                    </div>
                    <div class="bar"><i style=bar_style></i></div>
                </li>
            }
            .into_any()
        }
    }
}

/// Copy `text` to the clipboard.
///
/// The async Clipboard API (`navigator.clipboard`) only exists in secure
/// contexts (HTTPS or `localhost`). When a peer opens the share link on another
/// device over plain HTTP on the LAN, it is `undefined`, so we fall back to a
/// transient off-screen `<textarea>` + the legacy `document.execCommand("copy")`,
/// which works in non-secure contexts too.
fn copy_to_clipboard(text: &str) {
    let Some(win) = web_sys::window() else {
        return;
    };
    let clipboard = win.navigator().clipboard();
    let cb_val: wasm_bindgen::JsValue = clipboard.clone().into();
    if cb_val.is_undefined() || cb_val.is_null() {
        legacy_copy(&win, text);
    } else {
        // Fire-and-forget; the returned Promise is intentionally ignored.
        let _ = clipboard.write_text(text);
    }
}

/// Synchronous clipboard copy for non-secure contexts, where
/// `navigator.clipboard` is unavailable.
fn legacy_copy(win: &web_sys::Window, text: &str) {
    let Some(doc) = win.document() else {
        return;
    };
    let Some(body) = doc.body() else {
        return;
    };
    let Ok(area) = doc
        .create_element("textarea")
        .and_then(|el| {
            el.dyn_into::<web_sys::HtmlTextAreaElement>()
                .map_err(wasm_bindgen::JsValue::from)
        })
    else {
        return;
    };
    area.set_value(text);
    // Keep it off-screen so selecting it does not scroll or flash the page.
    let _ = area.set_attribute(
        "style",
        "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;",
    );
    let _ = area.set_attribute("readonly", "");
    if body.append_child(&area).is_err() {
        return;
    }
    area.select();
    if let Ok(html_doc) = doc.dyn_into::<web_sys::HtmlDocument>() {
        let _ = html_doc.exec_command("copy");
    }
    let _ = body.remove_child(&area);
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
    use super::{fmt_size, fmt_speed, Status};

    #[test]
    fn formats_human_sizes() {
        assert_eq!(fmt_size(0.0), "0 B");
        assert_eq!(fmt_size(512.0), "512 B");
        assert_eq!(fmt_size(1024.0), "1.0 KB");
        assert_eq!(fmt_size(1536.0), "1.5 KB");
        assert_eq!(fmt_size(1_048_576.0), "1.0 MB");
        assert_eq!(fmt_size(1_073_741_824.0), "1.0 GB");
    }

    #[test]
    fn speed_appends_per_second_suffix() {
        assert_eq!(fmt_speed(0.0), "0 B/s");
        assert_eq!(fmt_speed(1536.0), "1.5 KB/s");
        assert_eq!(fmt_speed(1_048_576.0), "1.0 MB/s");
    }

    #[test]
    fn status_labels_cover_every_variant() {
        assert_eq!(Status::Idle.label(), "Idle");
        assert_eq!(Status::WaitingForPeer.label(), "Waiting for peer to join…");
        assert_eq!(Status::Connecting.label(), "Connecting…");
        assert_eq!(Status::Connected.label(), "Connected — ready to transfer");
        assert_eq!(Status::PeerLeft.label(), "Peer disconnected");
        assert_eq!(Status::RoomFull.label(), "Room is full");
        assert_eq!(Status::RoomNotFound.label(), "Room not found or expired");
        assert_eq!(
            Status::Error("boom".into()).label(),
            "Couldn't establish direct connection: boom"
        );
    }
}
