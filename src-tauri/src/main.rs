#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

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
/// it caused a transition, emits `companion-state-changed`.
///
/// This is the seam the real watcher (task "Watcher/парсер транскрипта",
/// still open) plugs into once it exists — for now it can also be called
/// manually from the frontend devtools with a fake line to sanity-check the
/// wiring end to end without a real transcript file yet.
#[tauri::command]
fn ingest_transcript_line(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    line: serde_json::Value,
) -> Result<Option<String>, String> {
    let mut machine = state.machine.lock().map_err(|e| e.to_string())?;
    let Some(next) = machine.apply(&line) else {
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

const CHAT_WIDTH_LOGICAL: f64 = 320.0;
const CHAT_HEIGHT_LOGICAL: f64 = 420.0;

/// Where the chat popup should sit relative to the companion right now:
/// centered above its head, or below if that would run off the top of the
/// screen. Shared by `toggle_chat_window` (initial placement) and the
/// main-window `Moved` handler (keeps it pinned as the companion moves,
/// e.g. while roaming) so the two never compute this differently.
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
        .build()
        .map_err(|e| e.to_string())?;

    win.set_position(position).map_err(|e| e.to_string())?;

    Ok(())
}

/// Sends one chat message to a real Claude Code CLI (`claude -p`) and
/// returns its reply. Each call is a real API request with real cost --
/// confirmed live: a trivial "reply with exactly: pong" cost $0.057.
/// There's no rate limiting or cost guard here yet; that's a deliberate
/// gap, not an oversight -- flagging it rather than silently shipping
/// something that spends money unattended.
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

    let existing_session = chat.session_id.lock().map_err(|e| e.to_string())?.clone();
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

            // Keep the chat popup pinned to the companion instead of getting
            // left behind: every time the main window moves (manual drag or
            // the roaming timer's setPosition calls), re-run the same
            // placement math toggle_chat_window used and slide the chat
            // window along with it, but only while it's actually open.
            let app_handle = app.handle().clone();
            window.on_window_event(move |event| {
                let WindowEvent::Moved(_) = event else {
                    return;
                };
                let Some(main_window) = app_handle.get_webview_window(MAIN_WINDOW_LABEL) else {
                    return;
                };
                let Some(chat_window) = app_handle.get_webview_window(CHAT_WINDOW_LABEL) else {
                    return;
                };
                if !chat_window.is_visible().unwrap_or(false) {
                    return;
                }
                if let Ok(position) = chat_position_for(&main_window) {
                    let _ = chat_window.set_position(position);
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
