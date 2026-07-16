import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const companion = document.getElementById("companion");

const STATES = [
  "idle",
  "roaming",
  "thinking",
  "typing",
  "waitingForInput",
  "error",
  "done",
];

function applyState(state) {
  for (const s of STATES) companion.classList.remove(`state-${s}`);
  companion.classList.add(`state-${state}`);
}

// Backend emits companion-state-changed once it's wired to a real Claude Code
// watcher (Notion tasks "Watcher/парсер транскрипта" + "Источник live-статуса").
listen("companion-state-changed", (event) => {
  applyState(event.payload.state);
});

// Placeholder click handler — task "UI чат-попапа" wires this up for real.
companion.addEventListener("click", () => {
  console.log("Companion clicked");
});

applyState("idle");
