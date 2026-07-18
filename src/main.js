// Use global Tauri API (withGlobalTauri: true in tauri.conf.json)
// No ES module imports needed — works without a bundler.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow, currentMonitor } = window.__TAURI__.window;
const { LogicalPosition } = window.__TAURI__.dpi;

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

// States where nothing is happening backend-side, so it's fine for the
// companion to wander off on its own. Everything else (thinking/typing/
// error) should hold still -- "roaming" itself is never sent by the
// backend, it's purely a local idle-timeout behavior layered on top.
const ROAM_ELIGIBLE_STATES = new Set(["idle", "waitingForInput"]);

const ROAM_IDLE_DELAY_MS = 20_000; // start wandering after this long at rest
const ROAM_MOVE_INTERVAL_MS = 6_000; // then relocate on this cadence
const WINDOW_SIZE = 96; // keep in sync with tauri.conf.json window width/height
const SCREEN_MARGIN = 20;

const win = getCurrentWindow();
let idleTimer = null;
let roamInterval = null;

function applyState(state) {
  for (const s of STATES) companion.classList.remove(`state-${s}`);
  companion.classList.add(`state-${state}`);
}

function stopRoaming() {
  if (idleTimer) {
    clearTimeout(idleTimer);
    idleTimer = null;
  }
  if (roamInterval) {
    clearInterval(roamInterval);
    roamInterval = null;
  }
}

async function moveToRandomSpot() {
  const monitor = await currentMonitor();
  if (!monitor) return;
  const scale = monitor.scaleFactor || 1;
  const width = monitor.size.width / scale;
  const height = monitor.size.height / scale;
  const maxX = Math.max(SCREEN_MARGIN, width - WINDOW_SIZE - SCREEN_MARGIN);
  const maxY = Math.max(SCREEN_MARGIN, height - WINDOW_SIZE - SCREEN_MARGIN);
  const x = SCREEN_MARGIN + Math.random() * (maxX - SCREEN_MARGIN);
  const y = SCREEN_MARGIN + Math.random() * (maxY - SCREEN_MARGIN);
  await win.setPosition(new LogicalPosition(x, y)).catch((err) => console.error("setPosition failed", err));
}

function startRoamIdleCountdown() {
  stopRoaming();
  idleTimer = setTimeout(() => {
    applyState("roaming");
    moveToRandomSpot();
    roamInterval = setInterval(moveToRandomSpot, ROAM_MOVE_INTERVAL_MS);
  }, ROAM_IDLE_DELAY_MS);
}

// Backend state changes -- real Claude Code activity (see
// docs/claude-code-transcript.md for the transcript -> state mapping).
listen("companion-state-changed", (event) => {
  const state = event.payload.state;
  stopRoaming();
  applyState(state);
  if (ROAM_ELIGIBLE_STATES.has(state)) startRoamIdleCountdown();
});

// --- Drag-to-move + click-to-open-chat ---
let mouseDownAt = null;

companion.addEventListener("mousedown", (e) => {
  e.preventDefault();
  stopRoaming();
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
startRoamIdleCountdown();
console.log("✅ Desktop Companion JS loaded");
