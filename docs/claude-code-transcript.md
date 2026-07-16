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

Verified against the real 626-line transcript of this very session: 266
transitions, ending in `typing` (accurate — mid-turn while writing this).
15 error transitions detected, matching 15 real `is_error: true` tool
results found independently via a separate script. See git log / run
`node tools/transcript-watcher/derive-state.mjs <path>` yourself.

## Open questions / not yet resolved

- `roaming` and `idle` aren't derivable from the transcript at all — they're
  presentation-layer choices (e.g. "waitingForInput for >N seconds ⇒ start
  roaming animation"), not signals from Claude Code itself.
- `done` has no clear signal in this schema. Might not be a real distinct
  state, or might map to a specific tool (e.g. a task-completion marker) we
  haven't seen an example of yet. Left unresolved rather than guessed.
- Multi-session / multi-project: a real desktop companion needs to pick
  *which* transcript file to watch (most-recently-modified under
  `~/.claude/projects/**`, most likely) — not designed yet.
- Hook events (mentioned in the original plan) haven't been investigated —
  this research only covers the transcript file, which turned out to be
  sufficient for a first pass.
