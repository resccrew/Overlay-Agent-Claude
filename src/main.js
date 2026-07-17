// Use global Tauri API (withGlobalTauri: true in tauri.conf.json)
// No ES module imports needed — works without a bundler.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

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

const win = getCurrentWindow();

function applyState(state) {
  for (const s of STATES) companion.classList.remove(`state-${s}`);
  companion.classList.add(`state-${state}`);
}

// Backend state changes
listen("companion-state-changed", (event) => {
  const state = event.payload.state;
  applyState(state);
});

// --- Drag-to-move + click-to-open-chat ---
let mouseDownAt = null;

companion.addEventListener("mousedown", (e) => {
  e.preventDefault();
  mouseDownAt = { x: e.screenX, y: e.screenY };
});

companion.addEventListener("mousemove", (e) => {
  if (!mouseDownAt) return;
  const dx = Math.abs(e.screenX - mouseDownAt.x);
  const dy = Math.abs(e.screenY - mouseDownAt.y);
  if (dx > 3 || dy > 3) {
    mouseDownAt = null;
    win.startDragging().catch(() => {});
  }
});

companion.addEventListener("mouseup", () => {
  if (!mouseDownAt) return;
  mouseDownAt = null;
  invoke("toggle_chat_window").catch((err) =>
    console.error("toggle_chat_window failed", err)
  );
});

applyState("idle");
console.log("✅ Desktop Companion JS loaded");
