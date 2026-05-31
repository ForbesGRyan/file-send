mod app;
mod filetype;
mod protocol;
mod qr;
mod signaling;
mod transfer;
mod ui;
mod webrtc;

use app::App;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
