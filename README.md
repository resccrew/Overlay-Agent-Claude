# Desktop Companion

Always-on-top floating character (macOS/Windows/Linux) that reflects live Claude Code
session status (idle / thinking / typing / waiting / error / done), with a click-to-open
chat popup.

Plan and task tracking: Notion "Задачи" database (project: "Desktop-компаньон (macOS)").

## Status

Tauri v2 app: transparent, always-on-top, undecorated window with pixel-art sprites per
state, draggable via `data-tauri-drag-region`, a `set_click_through` command, and a
`companion-state-changed` event the frontend listens to for state-driven styling. A
background thread (`spawn_transcript_watcher` in `src-tauri/src/main.rs`) tails whichever
`~/.claude/projects/**/*.jsonl` transcript was most recently modified and feeds every new
line through `companion_state::StateMachine`, so the companion's state is driven by real
Claude Code activity, not just manual devtools calls.

Chose Tauri over Electron for lower memory/disk footprint, and over Swift/AppKit because
the requirement moved to cross-platform.

Compile-verified: `cargo check`/`cargo build` pass in `src-tauri/` given the webview deps
listed below (confirmed on Linux with `libwebkit2gtk-4.1-dev` etc. installed).

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

## Layout

- `src-tauri/src/main.rs` — Rust backend: window setup, `set_click_through`,
  `ingest_transcript_line` (manual/testing) commands, and the real
  `spawn_transcript_watcher` background thread
- `crates/companion-state/src/watcher.rs` — GUI-free transcript file discovery
  (`latest_transcript_file`) and tailing (`Tailer`)
- `src-tauri/tauri.conf.json` — window config (transparent, borderless, always-on-top,
  skip-taskbar)
- `src-tauri/capabilities/default.json` — Tauri v2 permission grants for the main window
- `src/` — frontend (plain HTML/CSS/JS, no bundler): pixel-art sprite per state, state →
  CSS class mapping, listens for `companion-state-changed`; `src/chat/` is the chat popup,
  wired to a real `claude -p` CLI session via `send_chat_message`. Its first message tries
  to `--resume` whichever local session `spawn_transcript_watcher` is already tailing, so
  the popup joins the same conversation the terminal is running rather than paying for a
  separate one. There's no way to make a chat message free -- it costs the same as typing
  it in the terminal would -- so `--max-budget-usd` (default `1.00`, override via
  `COMPANION_CHAT_MAX_BUDGET_USD`) caps a single reply's spend.

## Testing

- `cargo test` (from repo root or `crates/companion-state/`) — unit tests for the state
  machine and the transcript tailer/file-discovery logic, plus (in `src-tauri/src/main.rs`)
  a `tauri::test::mock_app()`-based test that `apply_line_and_emit` really reaches a real
  `listen()` handler through a real `AppHandle`, not just "the two halves look right by
  inspection"
- `npm test` (= `node --test src/main.test.mjs`) — loads the real `src/main.js` into a
  sandboxed `node:vm` context with a fake `window.__TAURI__`/DOM/timers and drives it with
  a deterministic fake clock (no real waiting); covers the roaming idle-timeout, that a
  real backend event always cancels/restarts it correctly, and the `roamGeneration` guard
  against an in-flight move landing after being superseded
- `tools/e2e-status-check.sh [path-to-transcript.jsonl]` — cross-checks the Rust
  `StateMachine` against the independent Node prototype
  (`tools/transcript-watcher/derive-state.mjs`) on the same real transcript (defaults to
  the most recently modified one under `~/.claude/projects`); a mismatch means the two
  implementations disagree on what a real Claude Code session means
- `tools/soak-test.sh` — runs the built app under Xvfb for an extended period against a
  synthetically-growing transcript, sampling RSS/CPU to catch leaks or busy-looping; see
  its header comment for how to read the output

None of the above drives a real webview end to end (Rust wiring and JS logic are each
verified in isolation, not the full `emit` → real-webview → DOM chain) — that would need a
WebDriver/`tauri-driver` setup, not yet done.

## Next up (see Notion for full breakdown)

- `roaming` has a sprite/CSS class but nothing currently triggers it — needs an
  idle-timeout decision in the frontend or watcher (see
  `docs/claude-code-transcript.md`'s open questions)
- `done` has no confirmed transcript signal yet (see same doc)
- Hook events (as an alternative/supplement to tailing the JSONL transcript) not
  investigated yet
