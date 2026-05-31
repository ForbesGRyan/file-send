# file-send Frontend (Brutalist Redesign) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the file-send client a brutalist visual identity (paper + ink + signal orange) plus copy-link, QR share, drag-active feedback, type badges, and a transfer-complete state — without touching `transfer.rs` or any WebRTC logic.

**Architecture:** All changes live in the `client` crate. Two pure helpers (`filetype`, `qr`) are unit-tested on the host target. The UI layer (`ui.rs`, `app.rs`) gets richer markup and wires the already-existing-but-no-op completion callbacks. A new `styles.css` (linked from `index.html`) carries the entire aesthetic. `transfer.rs`, `signaling.rs`, `webrtc.rs`, `protocol.rs`, and the server are untouched.

**Tech Stack:** Rust (edition 2024), Leptos 0.8 (CSR/WASM), Trunk, `qrcode` crate, web-sys Clipboard API.

---

## File Structure

- **Create** `crates/client/src/filetype.rs` — `file_kind(name, mime) -> &'static str` badge classifier. Pure, unit-tested.
- **Create** `crates/client/src/qr.rs` — `qr_svg(data) -> String` renders a QR matrix to a black/white SVG string. Pure, unit-tested.
- **Create** `crates/client/styles.css` — all brutalist styling.
- **Modify** `crates/client/Cargo.toml` — add `qrcode` dep; add `Navigator`/`Clipboard` web-sys features.
- **Modify** `crates/client/index.html` — link `styles.css` + Google Fonts.
- **Modify** `crates/client/src/main.rs` — declare `mod filetype; mod qr;`.
- **Modify** `crates/client/src/ui.rs` — `StatusBar` (dot+label), `FileProgress` (+`kind`,`done`), `ProgressList` (new row markup), new `ShareLink` component (copy + QR).
- **Modify** `crates/client/src/app.rs` — `drag_active` signal, QR signal, wire completion callbacks, mark rows done, render new components.

Build/verify commands used throughout:
- Unit tests (host): `cargo test -p client`
- WASM build (integration): `cd crates/client && trunk build`

---

## Task 1: QR-to-SVG helper (`qr.rs`)

**Files:**
- Create: `crates/client/src/qr.rs`
- Modify: `crates/client/Cargo.toml`
- Modify: `crates/client/src/main.rs`

- [ ] **Step 1: Add the `qrcode` dependency**

In `crates/client/Cargo.toml`, under `[dependencies]` (after the `gloo-timers` line), add:

```toml
qrcode = { version = "0.14", default-features = false }
```

(`default-features = false` drops the optional `image` dependency; we only use the low-level matrix API.)

- [ ] **Step 2: Declare the module**

In `crates/client/src/main.rs`, add the module declarations so the list reads:

```rust
mod app;
mod filetype;
mod protocol;
mod qr;
mod signaling;
mod transfer;
mod ui;
mod webrtc;
```

(`filetype` is added now too so both modules are declared once; its file is created in Task 2. To keep this task compiling on its own, create an empty `crates/client/src/filetype.rs` placeholder containing only `//! File-type badge classification. (implemented in Task 2)` — Task 2 overwrites it.)

- [ ] **Step 3: Write the failing test**

Create `crates/client/src/qr.rs` with ONLY the test module first:

```rust
//! Render a string to a minimal black-and-white SVG QR code.

#[cfg(test)]
mod tests {
    use super::qr_svg;

    #[test]
    fn encodes_link_as_svg() {
        let svg = qr_svg("https://file-send.app/#/room/abcd");
        assert!(svg.starts_with("<svg"), "should be an svg document");
        assert!(svg.contains("<rect"), "should contain module rects");
    }

    #[test]
    fn empty_string_on_unencodable_input() {
        // QR codes have a maximum capacity; an absurdly long payload fails.
        let huge = "x".repeat(10_000);
        assert_eq!(qr_svg(&huge), "");
    }
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p client qr::`
Expected: FAIL — compile error, `cannot find function qr_svg in this scope`.

- [ ] **Step 5: Implement `qr_svg`**

Add this above the `#[cfg(test)]` module in `crates/client/src/qr.rs`:

```rust
use qrcode::{Color, QrCode};

/// Encode `data` as a QR code and return a self-contained SVG string.
/// Returns an empty string if the data is too large to encode.
pub fn qr_svg(data: &str) -> String {
    let Ok(code) = QrCode::new(data.as_bytes()) else {
        return String::new();
    };
    let width = code.width();
    let quiet = 2usize; // quiet-zone modules around the code
    let dim = width + quiet * 2;
    let colors = code.to_colors();

    let mut rects = String::new();
    for y in 0..width {
        for x in 0..width {
            if colors[y * width + x] == Color::Dark {
                rects.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"1\" height=\"1\"/>",
                    x + quiet,
                    y + quiet
                ));
            }
        }
    }

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" \
         shape-rendering=\"crispEdges\">\
         <rect width=\"{dim}\" height=\"{dim}\" fill=\"#ffffff\"/>\
         <g fill=\"#0a0a0a\">{rects}</g></svg>"
    )
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p client qr::`
Expected: PASS (2 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/client/Cargo.toml crates/client/src/main.rs crates/client/src/qr.rs crates/client/src/filetype.rs
git commit -m "feat(client): add qr_svg helper for share-link QR codes"
```

---

## Task 2: File-type badge helper (`filetype.rs`)

**Files:**
- Modify (overwrite placeholder): `crates/client/src/filetype.rs`

- [ ] **Step 1: Write the failing test**

Replace the placeholder contents of `crates/client/src/filetype.rs` with ONLY the test module first:

```rust
//! Classify a filename / mime type into a short uppercase display badge.

#[cfg(test)]
mod tests {
    use super::file_kind;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(file_kind("report.pdf", ""), "PDF");
        assert_eq!(file_kind("photo.JPG", ""), "IMG");
        assert_eq!(file_kind("clip.mp4", ""), "VID");
        assert_eq!(file_kind("song.flac", ""), "AUD");
        assert_eq!(file_kind("archive.tar.gz", ""), "ZIP");
        assert_eq!(file_kind("notes.md", ""), "DOC");
    }

    #[test]
    fn falls_back_to_mime_then_file() {
        assert_eq!(file_kind("noext", "image/png"), "IMG");
        assert_eq!(file_kind("noext", "video/mp4"), "VID");
        assert_eq!(file_kind("mystery", "application/x-thing"), "FILE");
        assert_eq!(file_kind("mystery", ""), "FILE");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p client filetype::`
Expected: FAIL — `cannot find function file_kind in this scope`.

- [ ] **Step 3: Implement `file_kind`**

Add above the test module in `crates/client/src/filetype.rs`:

```rust
/// A short type badge (e.g. "PDF", "IMG") for a transfer row.
/// Tries the filename extension first, then the mime type, then "FILE".
pub fn file_kind(name: &str, mime: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "PDF",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" => "IMG",
        "mp4" | "mov" | "mkv" | "webm" | "avi" => "VID",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "AUD",
        "zip" | "tar" | "gz" | "rar" | "7z" => "ZIP",
        "doc" | "docx" | "txt" | "md" | "rtf" | "pages" => "DOC",
        _ => mime_kind(mime),
    }
}

fn mime_kind(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "IMG"
    } else if mime.starts_with("video/") {
        "VID"
    } else if mime.starts_with("audio/") {
        "AUD"
    } else {
        "FILE"
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p client filetype::`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/filetype.rs
git commit -m "feat(client): add file_kind badge classifier"
```

---

## Task 3: Stylesheet + fonts

**Files:**
- Create: `crates/client/styles.css`
- Modify: `crates/client/index.html`

- [ ] **Step 1: Create the stylesheet**

Create `crates/client/styles.css` with the full brutalist system:

```css
:root {
  --paper: #f2f0e9;
  --ink: #0a0a0a;
  --orange: #ff4d2e;
  --white: #ffffff;
}

* { box-sizing: border-box; }

body {
  margin: 0;
  background: var(--paper);
  color: var(--ink);
  font-family: "Space Grotesk", system-ui, sans-serif;
  /* subtle grain */
  background-image: radial-gradient(var(--ink) 0.5px, transparent 0.5px);
  background-size: 4px 4px;
  background-blend-mode: multiply;
}

.container {
  max-width: 720px;
  margin: 40px auto;
  padding: 30px;
  background: var(--paper);
  border: 3px solid var(--ink);
  position: relative;
}

/* Wordmark */
.wm {
  font-family: "Archivo Black", sans-serif;
  font-size: clamp(44px, 12vw, 64px);
  line-height: 0.82;
  text-transform: uppercase;
  letter-spacing: -3px;
  margin: 0;
}
.wm span { color: var(--orange); }
.tagline {
  font-family: "Space Mono", monospace;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 1px;
  margin: 10px 0 0;
  border-left: 3px solid var(--orange);
  padding-left: 8px;
}

/* Status */
.statusrow { display: flex; align-items: center; gap: 10px; margin: 24px 0; }
.dot { width: 14px; height: 14px; background: var(--orange); border: 2px solid var(--ink); flex-shrink: 0; }
.status { font-weight: 700; font-size: 14px; text-transform: uppercase; letter-spacing: 0.5px; }

/* Generic block */
.block {
  border: 3px solid var(--ink);
  background: var(--white);
  padding: 16px;
  margin-bottom: 18px;
  box-shadow: 6px 6px 0 var(--ink);
}
.label {
  font-family: "Space Mono", monospace;
  font-size: 10px; text-transform: uppercase; letter-spacing: 1px;
  margin: 0 0 10px;
}

/* Share block */
.share { display: flex; gap: 16px; align-items: flex-start; }
.share .left { flex: 1; min-width: 0; }
.linkrow { display: flex; border: 3px solid var(--ink); }
.linkrow input {
  flex: 1; min-width: 0; border: 0; padding: 10px;
  font-family: "Space Mono", monospace; font-size: 12px;
  background: var(--paper); color: var(--ink);
}
.copybtn {
  border: 0; border-left: 3px solid var(--ink);
  background: var(--orange); color: var(--ink);
  font-family: "Archivo Black", sans-serif; font-size: 12px;
  padding: 0 16px; text-transform: uppercase; cursor: pointer;
}
.copybtn:active { background: var(--ink); color: var(--paper); }
.note { font-family: "Space Mono", monospace; font-size: 10px; margin: 8px 0 0; }
.qrwrap { flex-shrink: 0; }
.qr { width: 104px; height: 104px; border: 3px solid var(--ink); background: var(--white); }
.qr svg { display: block; width: 100%; height: 100%; }
.qr-cap {
  font-family: "Space Mono", monospace; font-size: 9px;
  text-align: center; margin: 6px 0 0; text-transform: uppercase;
}

/* Drop zone */
.drop {
  border: 3px dashed var(--ink);
  background: repeating-linear-gradient(45deg, transparent, transparent 12px,
    rgba(255, 77, 46, 0.06) 12px, rgba(255, 77, 46, 0.06) 24px);
  padding: 34px 18px; text-align: center; margin-bottom: 18px;
  transition: background 0.1s, transform 0.1s;
}
.drop.active { background: var(--orange); border-style: solid; transform: translate(-2px, -2px); }
.drop b {
  font-family: "Archivo Black", sans-serif; font-size: 18px;
  text-transform: uppercase; display: block; letter-spacing: -0.5px;
}
.drop .sub { font-family: "Space Mono", monospace; font-size: 11px; display: block; margin-top: 6px; }
.drop input[type="file"] { margin-top: 14px; font-family: "Space Mono", monospace; font-size: 11px; }

/* Transfer rows */
.progress-list { list-style: none; padding: 0; margin: 0; }
.row {
  border: 3px solid var(--ink); background: var(--white);
  padding: 12px 14px; margin-bottom: 10px;
  display: flex; flex-direction: column; gap: 8px;
}
.row.done { background: var(--ink); color: var(--paper); }
.row .top { display: flex; justify-content: space-between; align-items: center; font-weight: 700; font-size: 13px; gap: 10px; }
.row .name { font-family: "Space Mono", monospace; }
.diricon { color: var(--orange); }
.row.done .diricon { color: var(--paper); }
.tag {
  font-family: "Space Mono", monospace; font-size: 9px;
  border: 2px solid currentColor; padding: 1px 5px; text-transform: uppercase;
}
.pct { font-family: "Archivo Black", sans-serif; font-size: 14px; white-space: nowrap; }
.bar { height: 14px; background: var(--paper); border: 2px solid var(--ink); }
.bar i { display: block; height: 100%; background: var(--orange); }
.row.done .bar { background: #333333; border-color: var(--paper); }
.row.done .bar i { background: var(--paper); }
```

- [ ] **Step 2: Link the stylesheet and fonts in `index.html`**

Replace the contents of `crates/client/index.html` with:

```html
<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>file-send</title>
    <link rel="preconnect" href="https://fonts.googleapis.com" />
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
    <link
      href="https://fonts.googleapis.com/css2?family=Archivo+Black&family=Space+Grotesk:wght@400;500;600;700&family=Space+Mono:wght@400;700&display=swap"
      rel="stylesheet"
    />
    <link data-trunk rel="css" href="styles.css" />
    <link data-trunk rel="rust" data-wasm-opt="z" />
  </head>
  <body></body>
</html>
```

- [ ] **Step 3: Verify the build copies the CSS**

Run: `cd crates/client && trunk build`
Expected: build succeeds; `crates/client/dist/` contains `styles-*.css` (hashed) and `index.html` references it. (No visual check yet — components still use old markup; that lands in Tasks 4–6.)

- [ ] **Step 4: Commit**

```bash
git add crates/client/styles.css crates/client/index.html
git commit -m "feat(client): brutalist stylesheet + display fonts"
```

---

## Task 4: Status bar + progress rows (`ui.rs`)

**Files:**
- Modify: `crates/client/src/ui.rs`

- [ ] **Step 1: Extend `FileProgress` and update `StatusBar`**

In `crates/client/src/ui.rs`, replace the `FileProgress` struct and the `StatusBar` component with:

```rust
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
```

(The `Status` enum and its `label()` impl above are unchanged — leave them as-is.)

- [ ] **Step 2: Rewrite `ProgressList` with the new row markup**

Replace the `ProgressList` component in `crates/client/src/ui.rs` with:

```rust
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
```

- [ ] **Step 3: Verify it compiles**

Run: `cd crates/client && trunk build`
Expected: build succeeds. (`app.rs` still constructs `FileProgress` without `kind`/`done` — it does NOT compile yet IF app.rs is touched, but at this point app.rs is unchanged and still uses the old field set, so the build WILL fail with "missing fields kind, done". That is expected and fixed in Task 6.)

Because of that cross-file dependency, this task is verified together with Task 6. Proceed to Task 5 without a separate green build here; do not commit a broken build on its own — **defer the commit for Task 4 until Step bundled in Task 6**.

> Note for the executor: Tasks 4, 5, and 6 form one compile unit (the `FileProgress` shape change ripples into `app.rs`). Make the edits for all three, then build once, then commit. The per-task commits below are written so that the Task 6 commit covers Tasks 4–6 together.

---

## Task 5: ShareLink component (`ui.rs`)

**Files:**
- Modify: `crates/client/src/ui.rs`
- Modify: `crates/client/Cargo.toml`

- [ ] **Step 1: Enable clipboard web-sys features**

In `crates/client/Cargo.toml`, inside the `[dependencies.web-sys]` `features = [ ... ]` list, add `"Navigator"` and `"Clipboard"` (e.g. append a line):

```toml
  "Navigator", "Clipboard",
```

- [ ] **Step 2: Add the `ShareLink` component**

At the bottom of `crates/client/src/ui.rs`, add:

```rust
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
```

- [ ] **Step 3: Add the web-sys import**

At the top of `crates/client/src/ui.rs`, the file currently imports only `leptos::prelude::*`. Add below it:

```rust
use leptos::prelude::*;
// web_sys is referenced fully-qualified (web_sys::window) in ShareLink; no extra use needed.
```

(No new `use` is required because `web_sys` is reachable as a crate; if the build reports `web_sys` unresolved, add `use web_sys;` — but it is a workspace dependency already pulled in transitively via leptos and declared in Cargo.toml.)

- [ ] **Step 4: (compile verified with Task 6 — see Task 4 note)**

Proceed to Task 6.

---

## Task 6: Wire it all up (`app.rs`) + green build

**Files:**
- Modify: `crates/client/src/app.rs`

- [ ] **Step 1: Update imports**

In `crates/client/src/app.rs`, update the `use crate::...` lines to:

```rust
use crate::filetype::file_kind;
use crate::qr::qr_svg;
use crate::signaling::Signaling;
use crate::transfer::{attach_receiver, send_files};
use crate::ui::{FileProgress, ProgressList, ShareLink, Status, StatusBar};
use crate::webrtc;
```

- [ ] **Step 2: Add the QR and drag-active signals**

Just after the existing signal declarations (the `let (progress, set_progress) = ...` line), add:

```rust
    let (qr, set_qr) = signal(String::new());
    let (drag_active, set_drag_active) = signal(false);
```

- [ ] **Step 3: Compute `kind` and `done` in `upsert_progress`**

Replace the `upsert_progress` closure with:

```rust
    // Helper to update one file's progress row.
    let upsert_progress = move |name: String, fraction: f64, direction: &'static str| {
        let kind = file_kind(&name, "");
        let done = fraction >= 1.0;
        set_progress.update(|list| {
            if let Some(item) =
                list.iter_mut().find(|p| p.name == name && p.direction == direction)
            {
                item.fraction = fraction;
                item.done = done;
            } else {
                list.push(FileProgress { name, fraction, direction, kind, done });
            }
        });
    };
```

- [ ] **Step 4: Use arrow-only directions and wire the receive completion callback**

In `wire_dc`, replace the `attach_receiver(...)` call with:

```rust
            let up = upsert_progress;
            attach_receiver(
                &channel,
                move |name, recv, total| {
                    let frac = if total > 0.0 { recv / total } else { 1.0 };
                    up(name, frac, "↓");
                },
                move |name| {
                    // Authoritative per-file completion on the receive side.
                    up(name, 1.0, "↓");
                },
            );
```

- [ ] **Step 5: Generate the QR when the room link is created**

In the `SignalMsg::Created { room }` arm, replace the body with:

```rust
                    SignalMsg::Created { room } => {
                        let origin = web_sys::window().unwrap().location().origin().unwrap();
                        let link = format!("{origin}/#/room/{room}");
                        set_qr.set(qr_svg(&link));
                        set_room_link.set(link);
                        set_status.set(Status::WaitingForPeer);
                    }
```

- [ ] **Step 6: Use the arrow direction on the send side**

In the `on_files` closure, change the send progress direction from `"↑ sending"` to `"↑"`:

```rust
                send_files(
                    channel.clone(),
                    files,
                    move |name, sent, total| {
                        let frac = if total > 0.0 { sent / total } else { 1.0 };
                        up(name, frac, "↑");
                    },
                    || {},
                );
```

- [ ] **Step 7: Clear drag-active state when files drop**

In the `on_drop` closure, add `set_drag_active.set(false);` as the first statement after `ev.prevent_default();`:

```rust
        move |ev: DragEvent| {
            ev.prevent_default();
            set_drag_active.set(false);
            let mut files = Vec::new();
```

- [ ] **Step 8: Replace the view body**

Replace the entire `view! { ... }` block at the end of `App` with:

```rust
    let drop_class = move || if drag_active.get() { "drop active" } else { "drop" };

    view! {
        <main class="container">
            <h1 class="wm">"File"<span>"·"</span><br/>"Send"</h1>
            <p class="tagline">"Peer-to-peer // bytes never touch a server"</p>

            <StatusBar status/>

            <Show when=move || !room_link.get().is_empty()>
                <ShareLink link=room_link qr=qr/>
            </Show>

            <div
                class=drop_class
                on:dragenter=move |ev: DragEvent| { ev.prevent_default(); set_drag_active.set(true); }
                on:dragover=move |ev: DragEvent| ev.prevent_default()
                on:dragleave=move |_| set_drag_active.set(false)
                on:drop=on_drop
            >
                <b>"Drop files here"</b>
                <span class="sub">"— or click to choose —"</span>
                <input type="file" multiple on:change=on_input_change />
            </div>

            <ProgressList items=progress/>
        </main>
    }
```

- [ ] **Step 9: Build the whole client (covers Tasks 4–6)**

Run: `cd crates/client && trunk build`
Expected: build succeeds with no errors. If `web_sys` is reported unresolved in `ui.rs`, add `use web_sys;` to the top of `ui.rs` and rebuild.

- [ ] **Step 10: Run unit tests to confirm nothing regressed**

Run: `cargo test -p client`
Expected: PASS (4 tests from Tasks 1–2).

- [ ] **Step 11: Commit (Tasks 4, 5, 6 together)**

```bash
git add crates/client/src/ui.rs crates/client/src/app.rs crates/client/Cargo.toml
git commit -m "feat(client): brutalist UI — status dot, badges, done state, copy + QR share, drag-active"
```

---

## Task 7: Manual browser verification

**Files:** none (verification only)

This step needs a human/agent looking at the running app. The chrome-devtools MCP skill or the `/run` skill can drive it.

- [ ] **Step 1: Build release client and start the server**

```bash
cd crates/client && trunk build --release && cd ../..
cargo run --release -p server
```

Then open `http://localhost:3000` in two browser tabs (the second uses the shared `#/room/<id>` link).

- [ ] **Step 2: Verify against the checklist**

Confirm each:
- Wordmark `FILE·SEND` renders in Archivo Black with the orange `·`; paper background with grain.
- Tab 1 shows the **share block**: readonly link, orange **COPY** button, and a scannable **QR** (the QR is a crisp black/white SVG, not blank).
- Clicking **COPY** places the link on the clipboard (paste elsewhere to confirm).
- Dragging a file over the drop zone turns it **orange/solid** (`.drop.active`); leaving reverts it.
- Sending a file shows a row with `↑`, mono filename, a type **badge** (e.g. `PDF`), a live percentage, and an orange bar.
- On the receiving tab the file downloads and its row flips to the **inverted `✓ DONE`** state; the sending row also reaches `✓ DONE` at 100%.
- Status text reflects real states (Waiting for peer → Connecting → Connected).

- [ ] **Step 3: Update README if needed**

If any developer-facing instruction changed (it should not have — no new build steps), update `README.md`. Otherwise note "no README change required."

- [ ] **Step 4: Final commit (only if anything changed in Step 3)**

```bash
git add README.md
git commit -m "docs: note brutalist UI in README"
```

---

## Self-Review

**Spec coverage:**
- Aesthetic tokens/fonts/borders/shadows/grain/hatch → Task 3 (`styles.css`).
- Wordmark + tagline → Task 6 Step 8.
- Status dot + label → Task 4 Step 1.
- Share block: link + COPY + QR → Tasks 1 (qr), 5 (component), 6 (generation/wiring).
- Drag-active drop zone → Task 6 Steps 7–8 + Task 3 `.drop.active`.
- Transfer rows: arrow, mono name, badge, %, bar → Task 4 Step 2.
- Type badge (PDF/IMG/…) → Task 2.
- Transfer-complete inverted `✓ DONE` (both directions) → Task 4 (markup) + Task 6 Steps 3–4 (done flag; receive via `on_complete`, send via fraction≥1.0).
- `transfer.rs` untouched → confirmed; no task modifies it.
- QR via `qrcode` crate, SVG injected → Tasks 1 + 5.
- Out-of-scope items (cancel, TURN, server, dark mode) → none introduced.

**Placeholder scan:** No "TBD"/"add error handling"/"similar to" — every code step shows full code. The only intentional placeholder is the one-line `filetype.rs` stub in Task 1 Step 2, overwritten in Task 2 Step 1.

**Type consistency:** `FileProgress { name, fraction, direction, kind, done }` is defined in Task 4 and constructed identically in Task 6. `file_kind(&str, &str) -> &'static str`, `qr_svg(&str) -> String`, `StatusBar(status)`, `ProgressList(items)`, `ShareLink(link, qr)` signatures match across tasks. Direction strings standardized to `"↑"`/`"↓"` in both `ui.rs` rendering and `app.rs` callers.
