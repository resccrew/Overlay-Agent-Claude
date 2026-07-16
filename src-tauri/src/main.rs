#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;
use tauri::{Emitter, Manager};

use companion_state::StateMachine;

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

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            machine: Mutex::new(StateMachine::new()),
        })
        .invoke_handler(tauri::generate_handler![
            set_click_through,
            ingest_transcript_line
        ])
        .setup(|app| {
            let window = app.get_webview_window("main").expect("main window must exist");
            window.set_always_on_top(true)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running desktop companion");
}
