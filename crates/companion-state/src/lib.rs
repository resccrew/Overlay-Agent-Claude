//! Maps Claude Code JSONL transcript lines to a companion character state.
//!
//! Deliberately has no Tauri/GTK/webview dependency so it compiles and tests
//! on any machine with just `cargo` — the rest of this workspace needs
//! webkit2gtk (Linux) or Xcode (macOS) to build, this crate doesn't.
//!
//! Port of `tools/transcript-watcher/derive-state.mjs`, which was verified
//! against a real live session transcript before this port existed. See
//! `docs/claude-code-transcript.md` for the schema notes.

use serde_json::Value;

pub mod watcher;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompanionState {
    Idle,
    Roaming,
    Thinking,
    Typing,
    WaitingForInput,
    Error,
    Done,
}

#[derive(Debug, Default)]
pub struct StateMachine {
    state: Option<CompanionState>,
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            state: Some(CompanionState::Idle),
        }
    }

    pub fn current(&self) -> CompanionState {
        self.state.unwrap_or(CompanionState::Idle)
    }

    /// Feed one parsed transcript line. Returns `Some(state)` if this line
    /// changed the state, `None` for metadata-only lines or no-op repeats.
    pub fn apply(&mut self, line: &Value) -> Option<CompanionState> {
        match line.get("type").and_then(Value::as_str) {
            Some("user") => self.apply_user(line),
            Some("assistant") => self.apply_assistant(line),
            Some("system") => self.apply_system(line),
            // "queue-operation", "ai-title", "mode", "attachment", "last-prompt":
            // metadata, not state signals (see docs/claude-code-transcript.md).
            _ => None,
        }
    }

    /// A `type:"system"`/`subtype:"stop_hook_summary"` line fires whenever a
    /// Stop hook runs, i.e. the agent turn has genuinely finished (not just
    /// produced a text reply -- see `apply_assistant`'s `WaitingForInput`,
    /// which fires earlier, right as that reply lands). Best-effort only:
    /// this line only exists for users who have a Stop hook configured at
    /// all, so plenty of real sessions will never emit it and `Done` simply
    /// won't fire for them -- there's no universal "the whole task is over"
    /// signal in the transcript format (see docs/claude-code-transcript.md).
    fn apply_system(&mut self, line: &Value) -> Option<CompanionState> {
        if line.get("subtype").and_then(Value::as_str) == Some("stop_hook_summary") {
            return self.set(CompanionState::Done);
        }
        None
    }

    fn apply_user(&mut self, line: &Value) -> Option<CompanionState> {
        let content = line.get("message")?.get("content")?.as_array()?;

        let has_error_result = content.iter().any(|c| {
            c.get("type").and_then(Value::as_str) == Some("tool_result")
                && c.get("is_error").and_then(Value::as_bool).unwrap_or(false)
        });
        if has_error_result {
            return self.set(CompanionState::Error);
        }

        let has_tool_result = content
            .iter()
            .any(|c| c.get("type").and_then(Value::as_str) == Some("tool_result"));
        if has_tool_result {
            return self.set(CompanionState::Typing); // still mid-turn
        }

        let has_real_text = content.iter().any(|c| {
            c.get("type").and_then(Value::as_str) == Some("text")
                && c.get("text")
                    .and_then(Value::as_str)
                    .map(|t| !t.trim().is_empty())
                    .unwrap_or(false)
        });
        if has_real_text {
            return self.set(CompanionState::Thinking); // fresh human turn just landed
        }

        None
    }

    fn apply_assistant(&mut self, line: &Value) -> Option<CompanionState> {
        let content = line.get("message")?.get("content")?.as_array()?;

        let has = |t: &str| content.iter().any(|c| c.get("type").and_then(Value::as_str) == Some(t));

        if has("tool_use") {
            return self.set(CompanionState::Typing);
        }
        if has("thinking") {
            return self.set(CompanionState::Thinking);
        }
        if has("text") {
            return self.set(CompanionState::WaitingForInput);
        }
        None
    }

    fn set(&mut self, next: CompanionState) -> Option<CompanionState> {
        let changed = self.state != Some(next);
        self.state = Some(next);
        changed.then_some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Synthetic fixtures shaped like real transcript lines, not copied from
    // any real conversation (this repo may end up on a public remote).

    #[test]
    fn starts_idle() {
        let m = StateMachine::new();
        assert_eq!(m.current(), CompanionState::Idle);
    }

    #[test]
    fn assistant_thinking_sets_thinking() {
        let mut m = StateMachine::new();
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "thinking", "thinking": "hmm" }] }
        });
        assert_eq!(m.apply(&line), Some(CompanionState::Thinking));
        assert_eq!(m.current(), CompanionState::Thinking);
    }

    #[test]
    fn assistant_tool_use_sets_typing() {
        let mut m = StateMachine::new();
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "tool_use", "name": "Bash" }] }
        });
        assert_eq!(m.apply(&line), Some(CompanionState::Typing));
    }

    #[test]
    fn assistant_text_sets_waiting_for_input() {
        let mut m = StateMachine::new();
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "text", "text": "done for now" }] }
        });
        assert_eq!(m.apply(&line), Some(CompanionState::WaitingForInput));
    }

    #[test]
    fn user_tool_result_error_sets_error() {
        let mut m = StateMachine::new();
        let line = json!({
            "type": "user",
            "message": { "content": [{ "type": "tool_result", "is_error": true, "content": "boom" }] }
        });
        assert_eq!(m.apply(&line), Some(CompanionState::Error));
    }

    #[test]
    fn user_tool_result_ok_sets_typing() {
        let mut m = StateMachine::new();
        let line = json!({
            "type": "user",
            "message": { "content": [{ "type": "tool_result", "is_error": false, "content": "ok" }] }
        });
        assert_eq!(m.apply(&line), Some(CompanionState::Typing));
    }

    #[test]
    fn user_real_text_sets_thinking() {
        let mut m = StateMachine::new();
        let line = json!({
            "type": "user",
            "message": { "content": [{ "type": "text", "text": "hey do the thing" }] }
        });
        assert_eq!(m.apply(&line), Some(CompanionState::Thinking));
    }

    #[test]
    fn stop_hook_summary_sets_done() {
        let mut m = StateMachine::new();
        // Synthetic, shaped like a real line but with a made-up hook command
        // (not copied from any real hook configuration).
        let line = json!({
            "type": "system",
            "subtype": "stop_hook_summary",
            "hookCount": 1,
            "hookInfos": [{ "command": "~/.claude/some-example-hook.sh" }]
        });
        assert_eq!(m.apply(&line), Some(CompanionState::Done));
    }

    #[test]
    fn system_line_without_stop_hook_subtype_is_noop() {
        let mut m = StateMachine::new();
        let line = json!({ "type": "system", "subtype": "something_else" });
        assert_eq!(m.apply(&line), None);
    }

    #[test]
    fn metadata_lines_are_noop() {
        let mut m = StateMachine::new();
        for t in ["queue-operation", "ai-title", "mode", "attachment", "last-prompt"] {
            let line = json!({ "type": t });
            assert_eq!(m.apply(&line), None, "type {t} should be a no-op");
        }
        assert_eq!(m.current(), CompanionState::Idle);
    }

    #[test]
    fn repeated_same_state_is_not_a_transition() {
        let mut m = StateMachine::new();
        let line = json!({
            "type": "assistant",
            "message": { "content": [{ "type": "tool_use", "name": "Bash" }] }
        });
        assert_eq!(m.apply(&line), Some(CompanionState::Typing));
        assert_eq!(m.apply(&line), None, "second identical tool_use shouldn't re-fire a transition");
    }

    #[test]
    fn full_turn_sequence_matches_expected_path() {
        let mut m = StateMachine::new();
        let lines = [
            json!({ "type": "user", "message": { "content": [{ "type": "text", "text": "fix the bug" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "thinking", "thinking": "..." }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "name": "Bash" }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "is_error": true, "content": "err" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "name": "Bash" }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "is_error": false, "content": "ok" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "fixed it" }] } }),
        ];
        let expected = [
            CompanionState::Thinking,
            CompanionState::Thinking, // no-op: same state, not asserted as a transition below
            CompanionState::Typing,
            CompanionState::Error,
            CompanionState::Typing,
            CompanionState::Typing, // no-op: tool_result ok while already Typing
            CompanionState::WaitingForInput,
        ];
        for (line, expected_state) in lines.iter().zip(expected.iter()) {
            m.apply(line);
            assert_eq!(m.current(), *expected_state);
        }
        assert_eq!(m.current(), CompanionState::WaitingForInput);
    }
}
