# Desktop Companion

Always-on-top floating character (macOS/Windows/Linux) that reflects live Claude Code
session status (idle / thinking / typing / waiting / error / done), with a click-to-open
chat popup.

Plan and task tracking: Notion "Задачи" database (project: "Desktop-компаньон (macOS)").

## Status

Skeleton stage — Tauri v2 app: transparent, always-on-top, undecorated window with a
placeholder circle, draggable via `data-tauri-drag-region`, a `set_click_through` command,
and a `companion-state-changed` event the frontend listens to for state-driven styling.
Not yet wired to real Claude Code activity or real sprites.

Chose Tauri over Electron for lower memory/disk footprint, and over Swift/AppKit because
the requirement moved to cross-platform. Not yet compile-verified — see below.

## Running

```sh
npm install
npm run tauri dev
```

Requires the Rust toolchain (`rustup`) plus platform webview deps:

- **Linux**: `webkit2gtk-4.1`, `libayatana-appindicator3`, `librsvg2`, `pkg-config`, build-essential
  (Debian/Ubuntu: `sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev pkg-config build-essential`)
- **macOS**: Xcode command line tools (`xcode-select --install`)
- **Windows**: WebView2 (preinstalled on modern Windows), MSVC build tools

Not compile-checked yet in this repo's dev sandbox — it has Rust but no sudo to install
`pkg-config`/`webkit2gtk-dev`, so `cargo check` fails at the `glib-sys` build step. First
person with a real dev machine (or sudo here) should run `cargo check` in `src-tauri/` and
fix whatever falls out.

## Layout

- `src-tauri/src/main.rs` — Rust backend: window setup, `set_click_through` and
  `set_companion_state` commands
- `src-tauri/tauri.conf.json` — window config (transparent, borderless, always-on-top,
  skip-taskbar)
- `src-tauri/capabilities/default.json` — Tauri v2 permission grants for the main window
- `src/` — frontend (plain HTML/CSS/JS for now): placeholder sprite, state → CSS class
  mapping, listens for `companion-state-changed`

## Next up (see Notion for full breakdown)

- Get a real `cargo check`/`cargo build` pass on a machine with the webview deps installed
- Wire `set_companion_state` to a real state machine driven by Claude Code activity
- Find and tail the Claude Code session source (JSONL transcript / hook events)
- Replace placeholder circle with pixel-art sprites per state
