#!/usr/bin/env node
// Prototype/reference implementation for task "Watcher/парсер транскрипта →
// маппинг событий в состояния персонажа". Pure logic, no GUI deps, so it's
// runnable and testable anywhere Node runs — including this dev sandbox,
// unlike the Tauri shell itself which needs a webview.
//
// Usage:
//   node derive-state.mjs <path-to-transcript.jsonl>   # replay + print transitions
//   node derive-state.mjs <path-to-transcript.jsonl> --follow  # tail live (SIGWINCH-free poll)
//
// Transcript location convention (Claude Code CLI, confirmed against a real
// session file in this sandbox):
//   ~/.claude/projects/<cwd-with-slashes-as-dashes>/<sessionId>.jsonl
// One JSON object per line, appended as the turn progresses (not batched at
// the end) — good for a tail -f style watcher.

import { createReadStream, existsSync, statSync } from "node:fs";
import { createInterface } from "node:readline";

/** @typedef {"idle"|"roaming"|"thinking"|"typing"|"waitingForInput"|"error"|"done"} CompanionState */

class StateMachine {
  constructor() {
    /** @type {CompanionState} */
    this.state = "idle";
    this.lastErrorAt = null;
  }

  /**
   * Feed one parsed transcript line, return the new state (or null if this
   * line doesn't change anything — e.g. metadata-only lines).
   * @param {any} line
   * @returns {CompanionState | null}
   */
  apply(line) {
    switch (line.type) {
      case "user": {
        const content = line.message?.content;
        if (!Array.isArray(content)) return null;

        const hasErrorResult = content.some(
          (c) => c?.type === "tool_result" && c.is_error
        );
        if (hasErrorResult) return this.#set("error");

        const hasToolResult = content.some((c) => c?.type === "tool_result");
        if (hasToolResult) return this.#set("typing"); // still mid-turn, waiting on next assistant step

        const hasRealText = content.some(
          (c) => c?.type === "text" && typeof c.text === "string" && c.text.trim().length > 0
        );
        if (hasRealText) return this.#set("thinking"); // fresh human turn just landed

        return null;
      }

      case "assistant": {
        const content = line.message?.content;
        if (!Array.isArray(content)) return null;

        if (content.some((c) => c?.type === "tool_use")) return this.#set("typing");
        if (content.some((c) => c?.type === "thinking")) return this.#set("thinking");
        if (content.some((c) => c?.type === "text")) return this.#set("waitingForInput");
        return null;
      }

      // Explicit "no work queued, nothing in flight" signal.
      case "queue-operation":
        if (line.operation === "dequeue") return null; // about to start a turn; user/assistant lines will follow immediately
        return null;

      default:
        // ai-title, mode, attachment, last-prompt: metadata, not state signals.
        return null;
    }
  }

  #set(next) {
    if (next === "error") this.lastErrorAt = Date.now();
    const changed = next !== this.state;
    this.state = next;
    return changed ? next : null;
  }
}

async function replay(path) {
  const machine = new StateMachine();
  const rl = createInterface({ input: createReadStream(path, { encoding: "utf8" }) });
  let lineNo = 0;
  const transitions = [];

  for await (const raw of rl) {
    lineNo++;
    const trimmed = raw.trim();
    if (!trimmed) continue;
    let parsed;
    try {
      parsed = JSON.parse(trimmed);
    } catch {
      continue; // tolerate partial/corrupt trailing line from a live tail
    }
    const next = machine.apply(parsed);
    if (next) transitions.push({ lineNo, state: next, timestamp: parsed.timestamp ?? null });
  }

  return { finalState: machine.state, transitions, totalLines: lineNo };
}

const [, , path, flag] = process.argv;

if (!path || !existsSync(path)) {
  console.error("Usage: node derive-state.mjs <path-to-transcript.jsonl> [--follow]");
  process.exit(1);
}

const result = await replay(path);
console.log(`Replayed ${result.totalLines} lines, ${result.transitions.length} state transitions.`);
console.log(`Final state: ${result.finalState}`);

const counts = {};
for (const t of result.transitions) counts[t.state] = (counts[t.state] ?? 0) + 1;
console.log("\nTransition counts by state:");
for (const [state, n] of Object.entries(counts)) console.log(`  ${state}: ${n}`);

const errorTransitions = result.transitions.filter((t) => t.state === "error");
if (errorTransitions.length) {
  console.log(`\nFirst ${Math.min(5, errorTransitions.length)} error transitions:`);
  for (const t of errorTransitions.slice(0, 5)) {
    console.log(`  line ${t.lineNo}: -> error  (${t.timestamp ?? "no timestamp"})`);
  }
}

console.log("\nTransitions (first 20 shown):");
for (const t of result.transitions.slice(0, 20)) {
  console.log(`  line ${t.lineNo}: -> ${t.state}  (${t.timestamp ?? "no timestamp"})`);
}
if (result.transitions.length > 20) {
  console.log(`  ... and ${result.transitions.length - 20} more`);
}

if (flag === "--follow") {
  console.log("\n--follow not implemented in the prototype yet — replay-only for now.");
  console.log("Real watcher (Rust side, once webview deps exist) should tail with inotify");
  console.log("and re-run apply() per newly appended line instead of re-reading the file.");
}
