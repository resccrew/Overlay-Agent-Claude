# Claude Code session transcript — format notes

Research for the Notion task "Источник live-статуса Claude Code: разведать
JSONL-транскрипт сессии и hook-события". Verified against a real, live
session transcript in the dev sandbox — not guessed from docs.

## Location

```
~/.claude/projects/<cwd-with-slashes-replaced-by-dashes>/<sessionId>.jsonl
```

Example seen in this sandbox: cwd `/home/openclaw/.openclaw/workspace` →
directory `-home-openclaw--openclaw-workspace`, one `.jsonl` file per session
(named by session UUID), sibling to a `<sessionId>/tool-results/` directory
holding large tool outputs referenced from the transcript.

The file is appended to line-by-line **as the turn progresses**, not written
once at the end — confirmed by replaying a 600+ line file mid-session and
getting a plausible in-progress final state. That makes `tail -f` / inotify
watching viable; no need to wait for turn completion to react.

## Line shapes seen

One JSON object per line. `type` field observed values, from a real session:

| type              | meaning                                                   |
|-------------------|------------------------------------------------------------|
| `user`            | human message OR tool results being fed back to the model |
| `assistant`       | model output: `thinking`, `tool_use`, or `text` blocks    |
| `queue-operation` | message enqueued/dequeued (metadata)                      |
| `last-prompt`     | latest prompt snapshot (metadata)                         |
| `ai-title`        | session title inference (metadata)                        |
| `mode`            | current mode, e.g. `"normal"` (metadata)                  |
| `attachment`      | attached file reference (metadata)                        |
| `system`          | hook lifecycle events, e.g. `subtype:"stop_hook_summary"` |

`user` and `assistant` lines carry `message.content`, an array of content
blocks. Relevant block `type`s:

- `assistant` → `thinking` — model is reasoning
- `assistant` → `tool_use` — model is invoking a tool
- `assistant` → `text` — model produced a reply chunk
- `user` → `tool_result` — result of a tool call, has `is_error: true/false`
- `user` → `text` — real human input (only when a human actually typed)

## Proposed state mapping

Implemented and tested in `tools/transcript-watcher/derive-state.mjs`
(pure Node, no GUI deps — runs in this sandbox even without a display):

- `assistant`/`thinking` → `thinking`
- `assistant`/`tool_use` → `typing`
- `user`/`tool_result` with `is_error: true` → `error`
- `user`/`tool_result` without error → `typing` (still mid-turn)
- `assistant`/`text` (no further tool_use in that message) → `waitingForInput`
- `user`/`text` (fresh human message) → `thinking` (new turn just started)
- `system`/`stop_hook_summary` → `done` (best-effort, see below)

Verified against the real 626-line transcript of this very session: 266
transitions, ending in `typing` (accurate — mid-turn while writing this).
15 error transitions detected, matching 15 real `is_error: true` tool
results found independently via a separate script. See git log / run
`node tools/transcript-watcher/derive-state.mjs <path>` yourself.

Implemented identically in both `tools/transcript-watcher/derive-state.mjs`
(Node prototype) and `crates/companion-state` (Rust, what `main.rs` actually
runs) -- `tools/e2e-status-check.sh` cross-checks the two against the same
real transcript on every run and fails if they disagree.

## Resolved: `done`

`type:"system"`, `subtype:"stop_hook_summary"` fires whenever a Stop hook
runs -- confirmed against this session's own real transcript (this repo's
sandbox has one configured: `~/.claude/stop-hook-git-check.sh`). That's a
genuine "the agent turn is fully over" signal, distinct from
`waitingForInput` (which fires earlier, right as the reply text lands, and
can't tell "turn over" apart from "still generating more before stopping").

Caveat: it's **best-effort, not universal**. Stop hooks are opt-in --
plenty of real Claude Code users have none configured, and for them this
line never appears, so `done` simply never fires. That's an acceptable
gap: `waitingForInput` already covers "turn finished, nothing more
expected" for everyone; `done` is a bonus signal for the subset of users
who have a Stop hook, not the primary mechanism.

## Resolved: `roaming` / `idle`

Not derivable from the transcript — confirmed, this is by design, not a
gap. They're a presentation-layer idle-timeout implemented in
`src/main.js` (`startRoamIdleCountdown`): after `ROAM_IDLE_DELAY_MS` (20s)
with no new backend state change while in `idle` or `waitingForInput`, the
frontend switches to the `roaming` sprite and relocates the window to a
random spot on the current monitor every `ROAM_MOVE_INTERVAL_MS` (6s),
using `currentMonitor()` + `setPosition()`. The backend never emits
`roaming` itself.

## Open questions / not yet resolved

- Multi-session / multi-project: `companion_state::watcher::ProjectsWatcher`
  picks the single most-recently-modified transcript under
  `~/.claude/projects/**`, so it follows whichever session the user
  touched last but can't show two sessions' states at once. Fine for the
  common case of one active session at a time; would need real design work
  to do better.
- Hook events beyond the Stop hook (PreToolUse, PostToolUse, etc.) haven't
  been investigated as additional state signals.
