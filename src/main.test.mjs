// Loads the *real* src/main.js (unmodified) into a sandboxed vm context with
// a fake window.__TAURI__/document/timers, and drives it deterministically
// with a hand-rolled fake timer queue instead of real sleeps. No test
// framework/bundler dependency beyond Node's built-ins (node:test, node:vm),
// consistent with this frontend having no build step at all.
//
// Run: node --test src/main.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const MAIN_JS_PATH = join(dirname(fileURLToPath(import.meta.url)), "main.js");

const ROAM_IDLE_DELAY_MS = 20_000;
const ROAM_MOVE_INTERVAL_MS = 6_000;

// --- Minimal fake timer queue (deterministic, no real waiting) ---
function makeFakeClock() {
  let now = 0;
  let nextId = 1;
  let timers = [];

  const setTimeout_ = (cb, delay) => {
    const id = nextId++;
    timers.push({ id, cb, triggerAt: now + delay, interval: null });
    return id;
  };
  const clearTimeout_ = (id) => {
    timers = timers.filter((t) => t.id !== id);
  };
  const setInterval_ = (cb, delay) => {
    const id = nextId++;
    timers.push({ id, cb, triggerAt: now + delay, interval: delay });
    return id;
  };
  const clearInterval_ = clearTimeout_;

  // Advances virtual time by `ms`, firing every due timer (including
  // recurring intervals) in order, exactly like a real event loop would.
  const advance = (ms) => {
    const target = now + ms;
    for (;;) {
      const due = timers.filter((t) => t.triggerAt <= target).sort((a, b) => a.triggerAt - b.triggerAt);
      if (due.length === 0) {
        now = target;
        return;
      }
      const t = due[0];
      now = t.triggerAt;
      if (t.interval != null) {
        t.triggerAt = now + t.interval;
      } else {
        timers = timers.filter((x) => x.id !== t.id);
      }
      t.cb();
    }
  };

  return { setTimeout: setTimeout_, clearTimeout: clearTimeout_, setInterval: setInterval_, clearInterval: clearInterval_, advance };
}

// Lets queued microtasks (the `await currentMonitor()` / `await
// win.setPosition()` chain inside moveToRandomSpot) actually run between
// fake-clock advances. Uses Node's real setImmediate -- a test-harness
// concern, not part of the sandboxed page code.
const flushMicrotasks = () => new Promise((resolve) => setImmediate(resolve));

function makeFakeElement(initialClass) {
  const classes = new Set([initialClass]);
  const listeners = {};
  return {
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
    },
    addEventListener: (type, handler) => {
      (listeners[type] ??= []).push(handler);
    },
    _classes: classes,
    _fire: (type, event) => {
      for (const h of listeners[type] ?? []) h(event);
    },
  };
}

// Builds one fresh sandbox: fake DOM, fake Tauri globals, fake clock, and
// runs the real main.js source inside it. Returns handles for driving/
// asserting on the loaded script from the test.
function loadMainJs() {
  const source = readFileSync(MAIN_JS_PATH, "utf8");
  const clock = makeFakeClock();

  const companion = makeFakeElement("state-idle");
  const document_ = { getElementById: (id) => (id === "companion" ? companion : null) };

  let listenCallback = null;
  const setPositionCalls = [];
  const startDraggingCalls = [];
  const invokeCalls = [];

  const monitor = { size: { width: 1280, height: 800 }, scaleFactor: 1 };

  const window_ = {
    __TAURI__: {
      core: { invoke: (cmd, args) => { invokeCalls.push([cmd, args]); return Promise.resolve(); } },
      event: { listen: (name, cb) => { listenCallback = cb; return Promise.resolve(() => {}); } },
      window: {
        getCurrentWindow: () => ({
          setPosition: (pos) => { setPositionCalls.push(pos); return Promise.resolve(); },
          startDragging: () => { startDraggingCalls.push(true); return Promise.resolve(); },
        }),
        currentMonitor: () => Promise.resolve(monitor),
      },
      dpi: {
        LogicalPosition: class LogicalPosition {
          constructor(x, y) { this.x = x; this.y = y; }
        },
      },
    },
  };

  const sandbox = {
    window: window_,
    document: document_,
    console,
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    setInterval: clock.setInterval,
    clearInterval: clock.clearInterval,
  };
  vm.createContext(sandbox);
  vm.runInContext(source, sandbox, { filename: "main.js" });

  return {
    companion,
    clock,
    setPositionCalls,
    startDraggingCalls,
    invokeCalls,
    emitBackendState: (state) => listenCallback({ payload: { state } }),
    mousedown: (x, y) => companion._fire("mousedown", { preventDefault() {}, screenX: x, screenY: y }),
    mousemove: (x, y) => companion._fire("mousemove", { screenX: x, screenY: y }),
    mouseup: () => companion._fire("mouseup", {}),
  };
}

test("starts idle and schedules a roam countdown on load", async () => {
  const page = loadMainJs();
  assert.ok(page.companion._classes.has("state-idle"));

  page.clock.advance(ROAM_IDLE_DELAY_MS - 1);
  await flushMicrotasks();
  assert.ok(page.companion._classes.has("state-idle"), "shouldn't roam a moment before the delay elapses");
  assert.equal(page.setPositionCalls.length, 0);

  page.clock.advance(1);
  await flushMicrotasks();
  assert.ok(page.companion._classes.has("state-roaming"), "should start roaming once idle delay elapses");
  assert.equal(page.setPositionCalls.length, 1);
});

test("roaming relocates repeatedly on the move interval", async () => {
  const page = loadMainJs();
  page.clock.advance(ROAM_IDLE_DELAY_MS);
  await flushMicrotasks();
  assert.equal(page.setPositionCalls.length, 1);

  page.clock.advance(ROAM_MOVE_INTERVAL_MS);
  await flushMicrotasks();
  assert.equal(page.setPositionCalls.length, 2);

  page.clock.advance(ROAM_MOVE_INTERVAL_MS);
  await flushMicrotasks();
  assert.equal(page.setPositionCalls.length, 3);
});

test("a non-roam-eligible backend state cancels the pending idle timer", async () => {
  const page = loadMainJs();
  page.clock.advance(5_000);
  page.emitBackendState("thinking");
  assert.ok(page.companion._classes.has("state-thinking"));

  // If the original idle timer were still alive, it would have fired well
  // before this point and flipped the class to roaming.
  page.clock.advance(ROAM_IDLE_DELAY_MS);
  await flushMicrotasks();
  assert.ok(page.companion._classes.has("state-thinking"), "should still be thinking, not roaming");
  assert.equal(page.setPositionCalls.length, 0);
});

test("a fresh roam-eligible state restarts its own countdown", async () => {
  const page = loadMainJs();
  page.emitBackendState("waitingForInput");
  assert.ok(page.companion._classes.has("state-waitingForInput"));

  page.clock.advance(ROAM_IDLE_DELAY_MS);
  await flushMicrotasks();
  assert.ok(page.companion._classes.has("state-roaming"));
  assert.equal(page.setPositionCalls.length, 1);
});

test("a real backend event during active roaming stops the move interval", async () => {
  const page = loadMainJs();
  page.clock.advance(ROAM_IDLE_DELAY_MS);
  await flushMicrotasks();
  assert.ok(page.companion._classes.has("state-roaming"));

  page.emitBackendState("error");
  assert.ok(page.companion._classes.has("state-error"));
  const callsAtInterrupt = page.setPositionCalls.length;

  page.clock.advance(ROAM_MOVE_INTERVAL_MS * 3);
  await flushMicrotasks();
  assert.equal(page.setPositionCalls.length, callsAtInterrupt, "roam interval should be dead, no further moves");
  assert.ok(page.companion._classes.has("state-error"), "should still show the real state, not roaming");
});

test("mousedown cancels a pending or active roam", async () => {
  const page = loadMainJs();
  page.clock.advance(ROAM_IDLE_DELAY_MS);
  await flushMicrotasks();
  assert.ok(page.companion._classes.has("state-roaming"));

  page.mousedown(100, 100);
  const callsAtMousedown = page.setPositionCalls.length;

  page.clock.advance(ROAM_MOVE_INTERVAL_MS * 3);
  await flushMicrotasks();
  assert.equal(page.setPositionCalls.length, callsAtMousedown, "no further roam moves after mousedown");
});

test("an in-flight move superseded mid-await is dropped, not applied postfactum", async () => {
  // Regression test for the roamGeneration guard: a roam-interval tick can
  // fire and start awaiting currentMonitor()/setPosition() in the same
  // instant a real backend event or drag interrupts it. Without the
  // guard, that in-flight move would still land after stopRoaming() ran.
  const page = loadMainJs();
  page.clock.advance(ROAM_IDLE_DELAY_MS);
  await flushMicrotasks();
  assert.equal(page.setPositionCalls.length, 1);

  page.clock.advance(ROAM_MOVE_INTERVAL_MS); // schedules another moveToRandomSpot(), not yet awaited past currentMonitor()
  page.emitBackendState("typing"); // supersedes it synchronously, before the microtask queue runs
  await flushMicrotasks();

  assert.equal(page.setPositionCalls.length, 1, "the superseded in-flight move must not call setPosition");
  assert.ok(page.companion._classes.has("state-typing"));
});
