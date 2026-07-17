#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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

/// Opens the chat popup window next to the companion, or focuses it if
/// already open. Creates it lazily on first click rather than at startup —
/// no point paying for a second webview before anyone asks for it.
///
/// UI-only right now (see src/chat/chat.js) — sending/receiving real
/// messages is task "Подключение чат-попапа к Claude Code", not started.
#[tauri::command]
fn toggle_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(CHAT_WINDOW_LABEL) {
        if existing.is_visible().map_err(|e| e.to_string())? {
            existing.hide().map_err(|e| e.to_string())?;
        } else {
            existing.show().map_err(|e| e.to_string())?;
            existing.set_focus().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    WebviewWindowBuilder::new(&app, CHAT_WINDOW_LABEL, WebviewUrl::App("chat/index.html".into()))
        .title("Companion Chat")
        .inner_size(320.0, 420.0)
        .resizable(true)
        .decorations(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            machine: Mutex::new(StateMachine::new()),
        })
        .invoke_handler(tauri::generate_handler![
            set_click_through,
            ingest_transcript_line,
            toggle_chat_window
        ])
        .setup(|app| {
            let window = app
                .get_webview_window(MAIN_WINDOW_LABEL)
                .expect("main window must exist");
            window.set_always_on_top(true)?;

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
