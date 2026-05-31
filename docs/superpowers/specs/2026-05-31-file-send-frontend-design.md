# file-send Frontend Design — Brutalist Redesign

Date: 2026-05-31
Status: Approved (design), pending implementation plan

## Goal

Give the existing file-send client a distinctive, production-grade visual
identity. The app works but ships no stylesheet — components reference CSS
classes that nothing defines. This design adds the stylesheet, the fonts, and a
focused set of UX improvements, without touching the WebRTC/transfer logic.

## Aesthetic Direction

**Brutalist · Paper + Signal Orange.**

Design tokens (CSS custom properties on `:root`):

| Token | Value | Use |
|-------|-------|-----|
| `--paper` | `#f2f0e9` | page background, inset bar track |
| `--ink` | `#0a0a0a` | text, borders, completed-row fill |
| `--orange` | `#ff4d2e` | accent: progress fill, copy button, dot, link rule |
| `--white` | `#ffffff` | block/field backgrounds |

Visual rules:
- 3px solid ink borders on every block.
- Hard offset shadows: `6px 6px 0 var(--ink)` (no blur).
- Subtle grain overlay (radial-dot, ~4% opacity) and a diagonal-hatch fill on
  the drop zone for texture.
- No rounded corners. Square dots, square badges.

Typography (Google Fonts; may be self-hosted later):
- `Archivo Black` — wordmark, headings, percentages, copy button.
- `Space Grotesk` — UI/body text.
- `Space Mono` — share link, file names, small uppercase labels.

## Layout

Single centered column inside a bordered "card on paper", max-width ~720px.

1. **Wordmark** — oversized `FILE·SEND`, the `·` in orange. Mono tagline:
   "Peer-to-peer // bytes never touch a server".
2. **Status row** — square orange dot + uppercase status text.
3. **Share block** — shown only when a room link exists. Readonly link field +
   orange `COPY` button; a QR code of the link sits beside it with a
   "scan on phone" caption. Note that the link expires when the tab closes.
4. **Drop zone** — dashed ink border, diagonal-hatch fill. Inverts to a filled
   active state while a drag is over it.
5. **Transfer list** — one row per file: direction arrow (↑/↓), mono filename,
   type badge, percentage (Archivo Black), thick progress bar. On completion the
   row inverts (ink background, paper text) and shows `✓ DONE`.

## Components & Changes

All changes are confined to the client crate. **`transfer.rs` is not modified.**

### `crates/client/index.html`
- Add `<link data-trunk rel="css" href="styles.css">` (Trunk copies it into
  `dist`).
- Add Google Fonts `<link>` tags (or a self-hosted fallback) for the three
  families above.

### `crates/client/styles.css` (new)
- All tokens, layout, block/shadow/border styling, drop-zone states, transfer
  rows (in-flight and done), share block, QR framing.

### `crates/client/src/ui.rs`
- `StatusBar` — render square dot + uppercase label. Keep the `Status` enum and
  its `label()` as-is.
- `FileProgress` — add a `done: bool` field and a derived `kind` badge
  (PDF / IMG / ZIP / DOC / file) computed from filename extension or mime.
- `ProgressList` — render the new row markup: arrow, mono name, badge, percent,
  bar; apply a `done` class when `done` is true (showing `✓ DONE`).
- New `ShareLink` component — readonly link input, `COPY` button, and a QR block
  that injects a generated SVG via `inner_html`.

### `crates/client/src/app.rs`
- Add a `drag_active` signal; toggle it on `dragenter`/`dragover` vs
  `dragleave`/`drop`; pass to the drop zone for the active state.
- Add a copy-to-clipboard handler for the share link (navigator.clipboard).
- Wire the currently-no-op completion callbacks:
  - Receive: `on_complete(name)` → mark that file's row `done`.
  - Send: a file is `done` when its progress fraction reaches 1.0 (works without
    touching `transfer.rs`); `on_done()` may drive an aggregate "all sent" cue.
- Generate the QR SVG string from the room link and feed it to `ShareLink`.

### `crates/client/Cargo.toml`
- Add the `qrcode` crate (pure-Rust, compiles to WASM) with SVG rendering.
  Render the room link to an SVG string; the `ShareLink` component injects it.

## Type Badge

Each transfer row shows a small badge (PDF / IMG / ZIP / DOC / FILE) derived
from the filename extension or mime type. Display-only; no logic impact.

## Out of Scope

- Per-file cancel/abort (would require `transfer.rs` changes).
- TURN relay / connectivity changes.
- Server-side changes.
- Dark mode / theme switching.

## Success Criteria

- `trunk build` succeeds; the styled UI renders with the chosen palette/fonts.
- Status, drop zone, share link, and progress reflect real app state.
- Drag-active state visibly changes the drop zone.
- COPY puts the link on the clipboard; QR encodes the same link and scans on a
  phone.
- A finished transfer shows the inverted `✓ DONE` row, both directions.
- WebRTC/transfer behavior is unchanged from current `main`.
