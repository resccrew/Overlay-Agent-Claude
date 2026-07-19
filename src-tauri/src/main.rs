#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
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

/// What `send_chat_message` hands back to the chat UI: the reply text plus
/// whatever session id is now active, so the frontend can show a real
/// session label (short id + message count) instead of a fake one.
#[derive(serde::Serialize)]
struct ChatReply {
    text: String,
    session_id: Option<String>,
}

#[tauri::command]
fn set_click_through(window: tauri::Window, enabled: bool) -> Result<(), String> {
    window.set_ignore_cursor_events(enabled).map_err(|e| e.to_string())
}

/// Feeds one already-parsed transcript line into the state machine and, if
/// it caused a transition, emits `companion-state-changed`. Shared by the
/// `ingest_transcript_line` command (manual devtools testing) and the real
/// background watcher thread spawned in `main()`.
fn apply_line_and_emit<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
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

// The "agent-chat" redesign (IDE-panel look: session rail + log + input +
// statusbar) is much wider than the old 320x420 bubble popup. Used as the
// *initial* size only now that the window is user-resizable -- see
// `chat_position_for`, which prefers the window's actual live size once it
// exists.
const CHAT_WIDTH_LOGICAL: f64 = 760.0;
const CHAT_HEIGHT_LOGICAL: f64 = 600.0;
// Small enough to still show the rail + a sliver of log/input, not so small
// the layout breaks.
const CHAT_MIN_WIDTH_LOGICAL: f64 = 420.0;
const CHAT_MIN_HEIGHT_LOGICAL: f64 = 320.0;

/// Where the chat popup should land when `toggle_chat_window` opens or
/// re-shows it: centered directly on the companion (chat center == companion
/// center), then clamped to the current monitor's bounds. The companion is
/// hidden while the chat is open, so covering its exact spot is fine and is
/// what "open it where the character is" means -- it no longer floats off
/// above the head in a different part of the screen. One-shot placement
/// only; the window is freely draggable afterward and nothing re-pins it.
///
/// `chat_window` is `None` only for the very first placement, before the
/// window exists yet, in which case the compile-time default size is the
/// best guess available. Every other call passes the window in so a user
/// resize is respected instead of the popup snapping back to 760x600.
fn chat_position_for(
    main_window: &tauri::WebviewWindow,
    chat_window: Option<&tauri::WebviewWindow>,
) -> Result<tauri::Position, String> {
    let main_pos = main_window.outer_position().map_err(|e| e.to_string())?;
    let main_size = main_window.outer_size().map_err(|e| e.to_string())?;
    let scale_factor = main_window.scale_factor().map_err(|e| e.to_string())?;

    let (chat_width_physical, chat_height_physical) = match chat_window {
        Some(w) => {
            let size = w.outer_size().map_err(|e| e.to_string())?;
            (size.width as i32, size.height as i32)
        }
        None => (
            (CHAT_WIDTH_LOGICAL * scale_factor) as i32,
            (CHAT_HEIGHT_LOGICAL * scale_factor) as i32,
        ),
    };

    // Center the chat on the companion's center point.
    let companion_center_x = main_pos.x + main_size.width as i32 / 2;
    let companion_center_y = main_pos.y + main_size.height as i32 / 2;
    let mut chat_x = companion_center_x - chat_width_physical / 2;
    let mut chat_y = companion_center_y - chat_height_physical / 2;

    // At 760 logical px wide, centering over the companion can push the
    // window past the screen edge once the companion is anywhere near one --
    // the old 320px popup never needed this. Clamp to the monitor it's
    // currently on so the chat window always stays fully visible.
    if let Ok(Some(monitor)) = main_window.current_monitor() {
        let m_pos = monitor.position();
        let m_size = monitor.size();
        let margin = 12;

        let min_x = m_pos.x + margin;
        let max_x = m_pos.x + m_size.width as i32 - chat_width_physical - margin;
        if max_x >= min_x {
            chat_x = chat_x.clamp(min_x, max_x);
        }

        let min_y = m_pos.y + margin;
        let max_y = m_pos.y + m_size.height as i32 - chat_height_physical - margin;
        if max_y >= min_y {
            chat_y = chat_y.clamp(min_y, max_y);
        }
    }

    Ok(tauri::Position::Physical(tauri::PhysicalPosition::new(chat_x, chat_y)))
}

/// Opens the chat popup window next to the companion, or focuses it if
/// already open. Creates it lazily on first click rather than at startup —
/// no point paying for a second webview before anyone asks for it.
///
/// The companion and the chat popup are mutually exclusive on screen: opening
/// chat hides the companion (nothing to click behind the popup anyway, and
/// two always-on-top windows stacked there just looks cluttered), closing it
/// (here, or via `close_chat_window`) brings the companion back. The tray
/// icon's "Show/Hide Companion" item is still a manual override on top of
/// this if it's ever needed.
#[tauri::command]
fn toggle_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    let main_window = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window not found".to_string())?;
    let existing = app.get_webview_window(CHAT_WINDOW_LABEL);

    let position = chat_position_for(&main_window, existing.as_ref())?;

    if let Some(existing) = existing {
        if existing.is_visible().map_err(|e| e.to_string())? {
            existing.hide().map_err(|e| e.to_string())?;
            main_window.show().map_err(|e| e.to_string())?;
        } else {
            existing.set_position(position).map_err(|e| e.to_string())?;
            existing.show().map_err(|e| e.to_string())?;
            existing.set_focus().map_err(|e| e.to_string())?;
            main_window.hide().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    let win = WebviewWindowBuilder::new(&app, CHAT_WINDOW_LABEL, WebviewUrl::App("chat/index.html".into()))
        .title("Companion Chat")
        .inner_size(CHAT_WIDTH_LOGICAL, CHAT_HEIGHT_LOGICAL)
        .min_inner_size(CHAT_MIN_WIDTH_LOGICAL, CHAT_MIN_HEIGHT_LOGICAL)
        // No OS decorations, so no native resize grip/border -- but on
        // macOS/Windows a borderless window with resizable(true) still
        // hit-tests its edges for the resize cursor and drag, no extra JS
        // handle needed. The CSS layout is flex-based (rail fixed width,
        // log/input/statusbar flexible) so it already adapts to any size.
        .resizable(true)
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
    main_window.hide().map_err(|e| e.to_string())?;

    Ok(())
}

/// Hides the chat popup and brings the companion back. Separate from
/// `toggle_chat_window` because the chat window's own close button always
/// means "close", not "toggle" -- and it needs to reach the main window too,
/// which a plain `currentWindow().hide()` in the chat's own JS can't do.
#[tauri::command]
fn close_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(chat) = app.get_webview_window(CHAT_WINDOW_LABEL) {
        chat.hide().map_err(|e| e.to_string())?;
    }
    if let Some(main) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        main.show().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Dollar cap passed to `claude -p --max-budget-usd` per chat message, so a
/// single reply can't run away and rack up an unbounded bill. Overridable
/// via `COMPANION_CHAT_MAX_BUDGET_USD` for anyone who wants a tighter or
/// looser limit than this default.
const DEFAULT_CHAT_MAX_BUDGET_USD: &str = "1.00";

/// One "here's what Claude is doing right now" line, pushed to the chat
/// window as `send_chat_message` reads the CLI's stream so the popup can
/// show live activity instead of sitting on a single spinner for however
/// long the reply takes -- same spirit as watching `claude` work in a
/// terminal.
#[derive(Clone, serde::Serialize)]
struct ChatActivity {
    text: String,
}

/// Turns one `--output-format stream-json` line into a short activity
/// label, or `None` for event types not worth surfacing in the popup
/// (session-start hooks, rate-limit bookkeeping, the init banner). A single
/// assistant turn can carry several content blocks (e.g. a thought and two
/// tool calls at once), so this returns one label per block, not just one
/// per line.
fn describe_stream_event(value: &serde_json::Value) -> Vec<String> {
    let truncate = |s: &str, n: usize| -> String {
        let short: String = s.chars().take(n).collect();
        if short.len() < s.len() { format!("{short}…") } else { short }
    };

    match value.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            let Some(content) = value.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_array()) else {
                return Vec::new();
            };
            content
                .iter()
                .filter_map(|block| match block.get("type").and_then(|t| t.as_str())? {
                    "thinking" => Some("думает…".to_string()),
                    "tool_use" => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                        let input = block.get("input").map(|i| i.to_string()).unwrap_or_default();
                        Some(format!("{name}({})", truncate(&input, 60)))
                    }
                    _ => None,
                })
                .collect()
        }
        Some("user") => {
            let Some(content) = value.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_array()) else {
                return Vec::new();
            };
            content
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(|t| t.as_str())? != "tool_result" {
                        return None;
                    }
                    let is_error = block.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false);
                    Some(if is_error { "✗ ошибка инструмента".to_string() } else { "✓ готово".to_string() })
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Sends one chat message to a real Claude Code CLI (`claude -p`) and
/// returns its reply, emitting a `chat-activity` event for every
/// intermediate step along the way (thinking, tool calls, tool results) so
/// the popup can show live progress instead of one silent wait. Each call
/// is a real API request with real cost -- confirmed live: a trivial
/// "reply with exactly: pong" cost $0.04-0.21 depending on caching. There
/// is no way to make a chat message free -- sending it here costs exactly
/// what typing the same message in the terminal would cost, since it's the
/// same underlying API usage either way.
///
/// The first message in a popup session starts a brand new `claude`
/// conversation; every message after that `--resume`s the session id the
/// *previous* call in this same popup returned, so the thread stays
/// coherent. A session id only ever gets used here if it came back from a
/// `claude -p` call this same function made, so it's always resumable (see
/// git history for why scanning `~/.claude/projects` for "whatever's most
/// recently active" doesn't work: `--resume` is scoped to the project
/// directory a session was created in).
///
/// `--max-budget-usd` caps runaway spend on a single reply (a hard stop,
/// not a soft warning -- the CLI itself enforces it).
///
/// Deliberately does NOT pass --dangerously-skip-permissions: this chat
/// popup should hold a conversation, not get silent shell access. Tool
/// calls that would need permission simply won't be grantable in this
/// non-interactive context.
#[tauri::command]
fn send_chat_message(app: tauri::AppHandle, chat: tauri::State<ChatSession>, message: String) -> Result<ChatReply, String> {
    eprintln!("[chat] send_chat_message: invoked, message={message:?}");

    let mut cmd = Command::new("claude");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Whenever this app itself is launched from inside an active Claude
    // Code session (e.g. `npm run tauri dev` from a Claude Code terminal --
    // true for every dev run so far), these vars leak into its environment
    // and get inherited by any child process by default. The spawned
    // `claude -p` here then sees CLAUDE_CODE_CHILD_SESSION=1 and an outer
    // CLAUDE_CODE_SESSION_ID, concludes it's a sub-agent of that outer
    // session, and hangs waiting to hand-shake with a coordinator that
    // isn't listening for it -- confirmed live: the old blocking
    // `cmd.output()` never returned, not even an error, just silence.
    // Stripping them makes this a genuinely independent top-level
    // invocation every time, chat popup or real terminal use.
    for var in [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_EXECPATH",
        "CLAUDE_CODE_SSE_PORT",
        "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING",
        "CLAUDE_CODE_ENABLE_TASKS",
        "CLAUDE_AGENT_SDK_VERSION",
    ] {
        cmd.env_remove(var);
    }
    // stream-json (+ --verbose, which some CLI versions require alongside
    // it in --print mode) gets us one JSON event per step as it happens,
    // instead of one blob only after the whole turn finishes.
    cmd.arg("-p").arg(&message).arg("--output-format").arg("stream-json").arg("--verbose");

    let max_budget = std::env::var("COMPANION_CHAT_MAX_BUDGET_USD")
        .unwrap_or_else(|_| DEFAULT_CHAT_MAX_BUDGET_USD.to_string());
    cmd.arg("--max-budget-usd").arg(&max_budget);

    let existing_session = chat.session_id.lock().map_err(|e| e.to_string())?.clone();
    if let Some(id) = &existing_session {
        cmd.arg("--resume").arg(id);
    }
    eprintln!("[chat] spawning: claude -p <message> --output-format stream-json --verbose --max-budget-usd {max_budget} {}",
        existing_session.as_deref().map(|id| format!("--resume {id}")).unwrap_or_default());

    let start = std::time::Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run `claude` CLI (is it installed and on PATH?): {e}"))?;

    let stdout = child.stdout.take().ok_or("failed to capture claude stdout")?;
    let mut stderr = child.stderr.take().ok_or("failed to capture claude stderr")?;

    // Drained on its own thread so a chatty stderr can't fill its OS pipe
    // buffer and deadlock the child while this thread is only reading
    // stdout below.
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    });

    let mut result_line: Option<String> = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        for activity in describe_stream_event(&value) {
            let _ = app.emit("chat-activity", ChatActivity { text: activity });
        }
        if value.get("type").and_then(|t| t.as_str()) == Some("result") {
            result_line = Some(trimmed.to_string());
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr_output = stderr_handle.join().unwrap_or_default();
    eprintln!(
        "[chat] claude exited after {:?}, status={:?}, stderr_len={}",
        start.elapsed(),
        status,
        stderr_output.len(),
    );

    if !status.success() {
        let err = stderr_output.trim().to_string();
        eprintln!("[chat] non-zero exit, stderr: {err}");
        return Err(err);
    }

    let Some(result_line) = result_line else {
        eprintln!("[chat] stream ended without a result event, stderr: {stderr_output}");
        return Err("claude CLI's output stream ended without a result".to_string());
    };

    let parsed: ClaudeCliResult = serde_json::from_str(&result_line).map_err(|e| {
        eprintln!("[chat] JSON parse failed: {e}\nraw result line: {result_line}");
        format!("failed to parse claude CLI output: {e}\nraw: {result_line}")
    })?;

    if let Some(id) = parsed.session_id {
        *chat.session_id.lock().map_err(|e| e.to_string())? = Some(id);
    }

    if parsed.is_error.unwrap_or(false) {
        return Err(parsed.result.unwrap_or_else(|| "claude CLI reported an error".to_string()));
    }

    let text = parsed.result.ok_or_else(|| "claude CLI returned no result text".to_string())?;
    let session_id = chat.session_id.lock().map_err(|e| e.to_string())?.clone();
    eprintln!("[chat] success, reply_len={}, session_id={session_id:?}", text.len());
    Ok(ChatReply { text, session_id })
}

/// Clears the tracked session id so the next `send_chat_message` call
/// starts a genuinely fresh `claude` conversation instead of `--resume`ing
/// the old one. Backs the chat UI's "Новая сессия" action (rail button and
/// the one on the error card).
#[tauri::command]
fn reset_chat_session(chat: tauri::State<ChatSession>) -> Result<(), String> {
    *chat.session_id.lock().map_err(|e| e.to_string())? = None;
    Ok(())
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
            close_chat_window,
            send_chat_message,
            reset_chat_session
        ])
        .setup(|app| {
            let window = app
                .get_webview_window(MAIN_WINDOW_LABEL)
                .expect("main window must exist");
            window.set_always_on_top(true)?;

            spawn_transcript_watcher(app.handle().clone());

            // The chat window used to be kept pinned to the companion via a
            // background poll loop. Dropped: the chat is now a fully
            // independent, freely draggable window (data-tauri-drag-region
            // in chat/index.html) -- continuously re-pinning it would have
            // fought a manual drag, snapping it back within ~50ms. It's
            // still placed near the companion on open via `chat_position_for`.

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

/// Verifies the actual Rust-side wiring end to end: `apply_line_and_emit`
/// (the exact function both `ingest_transcript_line` and
/// `spawn_transcript_watcher` call) really reaches a real `listen()`
/// handler through a real `AppHandle`, using Tauri's mock runtime -- not
/// just "it compiles and the two halves look right by inspection". Doesn't
/// cover the JS side (see `src/main.test.mjs` for that) or a real webview.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    use tauri::Listener;

    #[test]
    fn apply_line_and_emit_reaches_a_real_listener() {
        let app = tauri::test::mock_app();
        let handle = app.handle();

        let received: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_clone = received.clone();
        handle.listen("companion-state-changed", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            received_clone.lock().unwrap().push(payload["state"].as_str().unwrap().to_string());
        });

        let state = AppState { machine: Mutex::new(StateMachine::new()) };
        let line = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{ "type": "thinking", "thinking": "..." }] }
        });

        let result = apply_line_and_emit(handle, &state, &line);
        assert_eq!(result.unwrap(), Some("thinking".to_string()));
        assert_eq!(*received.lock().unwrap(), vec!["thinking".to_string()]);
    }

    #[test]
    fn apply_line_and_emit_is_a_noop_for_metadata_lines() {
        let app = tauri::test::mock_app();
        let handle = app.handle();

        let received: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let received_clone = received.clone();
        handle.listen("companion-state-changed", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            received_clone.lock().unwrap().push(payload["state"].as_str().unwrap().to_string());
        });

        let state = AppState { machine: Mutex::new(StateMachine::new()) };
        let line = serde_json::json!({ "type": "mode", "mode": "normal" });

        let result = apply_line_and_emit(handle, &state, &line);
        assert_eq!(result.unwrap(), None);
        assert!(received.lock().unwrap().is_empty(), "no event should have been emitted");
    }
}
