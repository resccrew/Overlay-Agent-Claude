#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{Emitter, Manager};

/// Mirrors the character state machine (task 3 in the Notion breakdown).
/// Kept as a plain string over the wire so the frontend owns rendering.
#[derive(Clone, serde::Serialize)]
struct StateChanged {
    state: String,
}

#[tauri::command]
fn set_click_through(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window.set_ignore_cursor_events(enabled).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_companion_state(app: tauri::AppHandle, state: String) -> Result<(), String> {
    app.emit("companion-state-changed", StateChanged { state })
        .map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![set_click_through, set_companion_state])
        .setup(|app| {
            let window = app.get_webview_window("main").expect("main window must exist");
            window.set_always_on_top(true)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running desktop companion");
}
