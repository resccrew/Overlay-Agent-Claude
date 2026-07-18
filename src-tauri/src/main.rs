#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use companion_state::StateMachine;

const CHAT_WINDOW_LABEL: &str = "chat";
const MAIN_WINDOW_LABEL: &str = "main";

/// Kept as a plain string over the wire so the frontend owns rendering and
/// doesn't need to know about the Rust enum's serde repr.
#[derive(Clone, serde::Serialize)]
struct StateChanged {
    state: String,
}

struct AppState {
    machine: Mutex<StateMachine>,
}

/// Tracks the Claude Code CLI session id across chat turns so `--resume`
/// keeps the conversation coherent instead of starting fresh every message.
#[derive(Default)]
struct ChatSession {
    session_id: Mutex<Option<String>>,
}

/// Subset of `claude -p --output-format json`'s result object we actually
/// use. Schema confirmed against a real invocation (see commit message) --
/// deliberately loose (all Option) since it's an undocumented-for-us CLI
/// output shape and we'd rather degrade than panic if a field is missing.
#[derive(serde::Deserialize)]
struct ClaudeCliResult {
    result: Option<String>,
    session_id: Option<String>,
    is_error: Option<bool>,
}

#[tauri::command]
fn set_click_through(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window.set_ignore_cursor_events(enabled).map_err(|e| e.to_string())
}

/// Feeds one already-parsed transcript line into the state machine and, if
/// it caused a transition, emits `companion-state-changed`. Shared by the
/// `ingest_transcript_line` command (manual devtools testing) and the real
/// background watcher thread spawned in `main()`.
fn apply_line_and_emit(
    app: &tauri::AppHandle,
    state: &AppState,
    line: &serde_json::Value,
) -> Result<Option<String>, String> {
    let mut machine = state.machine.lock().map_err(|e| e.to_string())?;
    let Some(next) = machine.apply(line) else {
        return Ok(None);
    };
    let state_str = serde_json::to_value(next)
        .map_err(|e| e.to_string())?
        .as_str()
        .unwrap_or("idle")
        .to_string();
    app.emit(
        "companion-state-changed",
        StateChanged {
            state: state_str.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(state_str))
}

/// Manual escape hatch: lets the frontend devtools (or a test) feed a fake
/// line through the exact same path the real watcher uses, without needing
/// a real transcript file on disk.
#[tauri::command]
fn ingest_transcript_line(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    line: serde_json::Value,
) -> Result<Option<String>, String> {
    apply_line_and_emit(&app, &state, &line)
}

/// `~/.claude/projects` (or `%USERPROFILE%\.claude\projects` on Windows),
/// overridable via `COMPANION_TRANSCRIPT_ROOT` for tests and for anyone
/// running Claude Code with a non-default `CLAUDE_CONFIG_DIR`.
fn transcript_projects_root() -> Option<std::path::PathBuf> {
    if let Ok(root) = std::env::var("COMPANION_TRANSCRIPT_ROOT") {
        return Some(std::path::PathBuf::from(root));
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(std::path::PathBuf::from(home).join(".claude").join("projects"))
}

/// How often to poll for newly-appended transcript lines. The rescan for
/// a *different* file having become the most-recently-modified one (the
/// user switched projects/sessions) is throttled separately inside
/// `ProjectsWatcher` itself.
const LINE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
const FILE_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Tails whichever Claude Code transcript is currently most active and
/// feeds every new line through the shared state machine, so
/// `companion-state-changed` reflects real Claude Code activity without
/// anyone having to call `ingest_transcript_line` by hand. Runs for the
/// lifetime of the app on a dedicated thread; all failures (missing
/// `~/.claude/projects`, a transcript that disappears mid-session, etc.)
/// are swallowed and retried on the next scan rather than crashing the app
/// -- the companion should degrade to looking idle, not take the window
/// down with it.
///
/// All the "which file, how to switch, how to tail" logic lives in
/// `companion_state::watcher::ProjectsWatcher` where it's covered by a
/// `cargo test` that doesn't need this GUI shell at all; this function is
/// just the thread + state-machine + `AppHandle::emit` wiring around it.
fn spawn_transcript_watcher(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let Some(projects_root) = transcript_projects_root() else {
            eprintln!("transcript watcher: could not determine HOME, watcher disabled");
            return;
        };

        let mut watcher = companion_state::watcher::ProjectsWatcher::new(projects_root, FILE_RESCAN_INTERVAL);

        loop {
            std::thread::sleep(LINE_POLL_INTERVAL);
            let lines = watcher.poll();
            if lines.is_empty() {
                continue;
            }
            let state = app_handle.state::<AppState>();
            for line in &lines {
                let _ = apply_line_and_emit(&app_handle, &state, line);
            }
        }
    });
}

const CHAT_WIDTH_LOGICAL: f64 = 320.0;
const CHAT_HEIGHT_LOGICAL: f64 = 420.0;

/// Where the chat popup should sit relative to the companion right now:
/// centered above its head, or below if that would run off the top of the
/// screen. Shared by `toggle_chat_window` (initial placement) and the
/// background poll loop in `main()` that keeps it pinned as the companion
/// moves, so the two never compute this differently.
fn chat_position_for(main_window: &tauri::WebviewWindow) -> Result<tauri::Position, String> {
    let main_pos = main_window.outer_position().map_err(|e| e.to_string())?;
    let main_size = main_window.outer_size().map_err(|e| e.to_string())?;
    let scale_factor = main_window.scale_factor().map_err(|e| e.to_string())?;

    let chat_width_physical = (CHAT_WIDTH_LOGICAL * scale_factor) as i32;
    let chat_height_physical = (CHAT_HEIGHT_LOGICAL * scale_factor) as i32;

    // Try to place it 20px above the companion's head
    let chat_x = main_pos.x + (main_size.width as i32 / 2) - (chat_width_physical / 2);
    let mut chat_y = main_pos.y - chat_height_physical - 20;

    // If it doesn't fit on the screen above the companion, place it below instead
    if chat_y < 20 {
        chat_y = main_pos.y + main_size.height as i32 + 20;
    }

    Ok(tauri::Position::Physical(tauri::PhysicalPosition::new(chat_x, chat_y)))
}

/// Opens the chat popup window next to the companion, or focuses it if
/// already open. Creates it lazily on first click rather than at startup —
/// no point paying for a second webview before anyone asks for it.
///
/// UI-only right now (see src/chat/chat.js) — sending/receiving real
/// messages is task "Подключение чат-попапа к Claude Code", not started.
#[tauri::command]
fn toggle_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    let main_window = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window not found".to_string())?;

    let position = chat_position_for(&main_window)?;

    if let Some(existing) = app.get_webview_window(CHAT_WINDOW_LABEL) {
        if existing.is_visible().map_err(|e| e.to_string())? {
            existing.hide().map_err(|e| e.to_string())?;
        } else {
            existing.set_position(position).map_err(|e| e.to_string())?;
            existing.show().map_err(|e| e.to_string())?;
            existing.set_focus().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    let win = WebviewWindowBuilder::new(&app, CHAT_WINDOW_LABEL, WebviewUrl::App("chat/index.html".into()))
        .title("Companion Chat")
        .inner_size(CHAT_WIDTH_LOGICAL, CHAT_HEIGHT_LOGICAL)
        .resizable(false)
        .decorations(false) // No OS decorations
        .transparent(true)  // Semi-transparent background
        .always_on_top(true)
        .skip_taskbar(true)
        // Without this, macOS draws its own native drop shadow around the
        // window's *actual* (rectangular) frame on top of the rounded glass
        // panel's own CSS box-shadow -- two overlapping shadows read as a
        // doubled/ghosted border under the panel. The main window already
        // gets `"shadow": false` from tauri.conf.json; this window is built
        // in Rust, so it needs the same thing set explicitly.
        .shadow(false)
        .build()
        .map_err(|e| e.to_string())?;

    win.set_position(position).map_err(|e| e.to_string())?;

    Ok(())
}

/// The session id of whichever Claude Code transcript is currently most
/// active -- i.e. exactly the session `spawn_transcript_watcher` is
/// tailing for the companion's status. Transcript files are named
/// `<sessionId>.jsonl` (see `docs/claude-code-transcript.md`), so the file
/// stem *is* the id `--resume` wants.
///
/// Used so the chat popup's first message joins the real, already-running
/// local `claude` session (same conversation the user sees in their
/// terminal) instead of starting a brand new, separately-billed one.
fn active_terminal_session_id() -> Option<String> {
    let projects_root = transcript_projects_root()?;
    let path = companion_state::watcher::latest_transcript_file(&projects_root)?;
    path.file_stem()?.to_str().map(str::to_string)
}

/// Dollar cap passed to `claude -p --max-budget-usd` per chat message, so a
/// single reply can't run away and rack up an unbounded bill. Overridable
/// via `COMPANION_CHAT_MAX_BUDGET_USD` for anyone who wants a tighter or
/// looser limit than this default.
const DEFAULT_CHAT_MAX_BUDGET_USD: &str = "1.00";

/// Sends one chat message to a real Claude Code CLI (`claude -p`) and
/// returns its reply. Each call is a real API request with real cost --
/// confirmed live: a trivial "reply with exactly: pong" cost $0.057-0.21
/// depending on whether a fresh system-prompt cache had to be created.
/// There is no way to make a chat message free -- sending it here costs
/// exactly what typing the same message in the terminal would cost, since
/// it's the same underlying API usage either way.
///
/// To avoid paying for *and forking* a whole separate conversation on top
/// of whatever the user already has open in a terminal, the very first
/// message tries to `--resume` the session `spawn_transcript_watcher` is
/// already tailing (see `active_terminal_session_id`) -- so the popup
/// continues that same real local session rather than starting a new one.
/// If no local session is currently active, it falls back to starting a
/// fresh one, same as before.
///
/// `--max-budget-usd` caps runaway spend on a single reply (a hard stop,
/// not a soft warning -- the CLI itself enforces it).
///
/// Deliberately does NOT pass --dangerously-skip-permissions: this chat
/// popup should hold a conversation, not get silent shell access. Tool
/// calls that would need permission simply won't be grantable in this
/// non-interactive context.
#[tauri::command]
fn send_chat_message(chat: tauri::State<ChatSession>, message: String) -> Result<String, String> {
    let mut cmd = Command::new("claude");
    cmd.stdin(std::process::Stdio::null());
    cmd.arg("-p").arg(&message).arg("--output-format").arg("json");

    let max_budget = std::env::var("COMPANION_CHAT_MAX_BUDGET_USD")
        .unwrap_or_else(|_| DEFAULT_CHAT_MAX_BUDGET_USD.to_string());
    cmd.arg("--max-budget-usd").arg(max_budget);

    let mut existing_session = chat.session_id.lock().map_err(|e| e.to_string())?.clone();
    if existing_session.is_none() {
        existing_session = active_terminal_session_id();
    }
    if let Some(id) = &existing_session {
        cmd.arg("--resume").arg(id);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("failed to run `claude` CLI (is it installed and on PATH?): {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout.find('{').unwrap_or(0);
    let json_str = &stdout[json_start..];
    let parsed: ClaudeCliResult = serde_json::from_str(json_str)
        .map_err(|e| format!("failed to parse claude CLI output: {e}\nraw: {stdout}"))?;

    if let Some(id) = parsed.session_id {
        *chat.session_id.lock().map_err(|e| e.to_string())? = Some(id);
    }

    if parsed.is_error.unwrap_or(false) {
        return Err(parsed.result.unwrap_or_else(|| "claude CLI reported an error".to_string()));
    }

    parsed.result.ok_or_else(|| "claude CLI returned no result text".to_string())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            machine: Mutex::new(StateMachine::new()),
        })
        .manage(ChatSession::default())
        .invoke_handler(tauri::generate_handler![
            set_click_through,
            ingest_transcript_line,
            toggle_chat_window,
            send_chat_message
        ])
        .setup(|app| {
            let window = app
                .get_webview_window(MAIN_WINDOW_LABEL)
                .expect("main window must exist");
            window.set_always_on_top(true)?;

            spawn_transcript_watcher(app.handle().clone());

            // Keep the chat popup pinned to the companion instead of getting
            // left behind. This is a poll loop, not a `WindowEvent::Moved`
            // listener, on purpose: dragging the companion goes through
            // `window.startDragging()` (a native OS drag session), and that
            // doesn't reliably surface timely Moved events through
            // tao/wry's normal callback -- confirmed live, the event-based
            // version left the chat window sitting still while the
            // companion visibly walked away from it. Polling the position
            // instead is immune to whatever's swallowing/delaying that
            // event, at the cost of a background thread.
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut last_main_pos: Option<(i32, i32)> = None;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));

                    let Some(main_window) = app_handle.get_webview_window(MAIN_WINDOW_LABEL) else {
                        continue;
                    };
                    let Some(chat_window) = app_handle.get_webview_window(CHAT_WINDOW_LABEL) else {
                        last_main_pos = None; // no chat yet -- force a fresh placement once it opens
                        continue;
                    };
                    if !chat_window.is_visible().unwrap_or(false) {
                        last_main_pos = None;
                        continue;
                    }

                    let Ok(pos) = main_window.outer_position() else {
                        continue;
                    };
                    let current = (pos.x, pos.y);
                    if last_main_pos == Some(current) {
                        continue;
                    }
                    last_main_pos = Some(current);

                    if let Ok(position) = chat_position_for(&main_window) {
                        let _ = chat_window.set_position(position);
                    }
                }
            });

            // Fallback access to the companion if it wanders off-screen while
            // roaming, or gets hidden some other way — click the tray icon to
            // bring it back.
            let toggle_item = MenuItem::with_id(app, "toggle_main", "Show/Hide Companion", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&toggle_item, &quit_item])?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("default window icon must be configured"))
                .menu(&tray_menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "toggle_main" => {
                        let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
                            return;
                        };
                        let is_visible = window.is_visible().unwrap_or(false);
                        if is_visible {
                            let _ = window.hide();
                        } else {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running desktop companion");
}
