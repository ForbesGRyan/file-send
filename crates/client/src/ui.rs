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
    pub direction: &'static str, // "↑ sending" / "↓ receiving"
}

#[component]
pub fn StatusBar(status: ReadSignal<Status>) -> impl IntoView {
    view! {
        <p class="status">{move || status.get().label()}</p>
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
                        view! {
                            <li>
                                <span>{p.direction}" "{p.name.clone()}</span>
                                <progress max="100" value=pct></progress>
                                <span>{pct}"%"</span>
                            </li>
                        }
                    })
                    .collect_view()
            }}
        </ul>
    }
}
