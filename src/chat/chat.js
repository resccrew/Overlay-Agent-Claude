const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const log = document.getElementById("log");
const form = document.getElementById("composer");
const input = document.getElementById("input");
const sendButton = form.querySelector('button[type="submit"]');
const titleEl = document.getElementById("ac-title");
const sessionMetaEl = document.getElementById("session-meta");
const statusCountEl = document.getElementById("status-count");
const newSessionBtn = document.getElementById("new-session-btn");

// Goes through the Rust command (not a plain currentWindow().hide()) so the
// companion, hidden while chat is open, comes back at the same time.
document.getElementById("close-btn").addEventListener("click", () => {
  invoke("close_chat_window").catch((err) => console.error("close_chat_window failed", err));
});

let messageCount = 0;
let currentSessionId = null;
let lastUserMessage = null;

function refreshSessionInfo() {
  const short = currentSessionId ? currentSessionId.slice(0, 6) : null;
  titleEl.textContent = short ? `claude agent — ${short}` : "claude agent";
  const label = `${short ?? "новая"} · ${messageCount} msgs`;
  sessionMetaEl.textContent = label;
  statusCountEl.textContent = `${messageCount} msgs`;
}

function appendUser(text) {
  const row = document.createElement("div");
  row.className = "ac-user";
  row.textContent = text;
  log.appendChild(row);
  log.scrollTop = log.scrollHeight;
  return row;
}

function appendAgentProse(text) {
  const row = document.createElement("div");
  row.className = "ac-agent";
  row.textContent = text;
  log.appendChild(row);
  return row;
}

function appendCodeBlock(lang, code) {
  const wrap = document.createElement("div");
  wrap.className = "ac-code";

  const head = document.createElement("div");
  head.className = "head";
  const label = document.createElement("span");
  label.textContent = lang || "code";
  const copy = document.createElement("button");
  copy.className = "copy";
  copy.type = "button";
  copy.textContent = "копировать";
  copy.addEventListener("click", () => {
    navigator.clipboard.writeText(code.trim()).then(() => {
      copy.textContent = "скопировано";
      setTimeout(() => (copy.textContent = "копировать"), 1200);
    });
  });
  head.appendChild(label);
  head.appendChild(copy);

  const pre = document.createElement("pre");
  pre.textContent = code.trim();

  wrap.appendChild(head);
  wrap.appendChild(pre);
  log.appendChild(wrap);
}

// Splits a reply on fenced code blocks (```lang\ncode\n```): prose renders
// as plain text, code renders in a labeled panel with a real copy button.
// No syntax highlighting -- a regex-based highlighter would misfire
// unpredictably across languages on live replies, so plain-but-correct
// beats colored-but-wrong.
function appendAgent(text) {
  const fence = /```(\w*)\n([\s\S]*?)```/g;
  let lastIndex = 0;
  let match;
  let sawCode = false;

  while ((match = fence.exec(text)) !== null) {
    sawCode = true;
    const before = text.slice(lastIndex, match.index).trim();
    if (before) appendAgentProse(before);
    appendCodeBlock(match[1], match[2]);
    lastIndex = fence.lastIndex;
  }

  const rest = text.slice(lastIndex).trim();
  if (rest || !sawCode) appendAgentProse(rest || text);
  log.scrollTop = log.scrollHeight;
}

// The label span is returned separately from the row so sendMessage can
// keep rewriting its text as `chat-activity` events arrive, instead of
// this being a static "claude думает…" that sits there unchanged for
// however long the reply takes.
function appendStatus(text) {
  const row = document.createElement("div");
  row.className = "ac-status";
  const dot = document.createElement("span");
  dot.className = "dot";
  dot.textContent = "●";
  const label = document.createElement("span");
  label.textContent = text;
  const caret = document.createElement("span");
  caret.className = "ac-caret";
  row.appendChild(dot);
  row.appendChild(label);
  row.appendChild(caret);
  log.appendChild(row);
  log.scrollTop = log.scrollHeight;
  return { row, label };
}

// Only one request is ever in flight at a time (the composer is disabled
// while sending), so a single shared reference to "the current status
// label" is enough -- no need to correlate activity events to a specific
// request.
let activeStatusLabel = null;

listen("chat-activity", (event) => {
  if (!activeStatusLabel) return;
  activeStatusLabel.textContent = event.payload.text;
  log.scrollTop = log.scrollHeight;
});

function appendError(message, { onRetry, onNewSession }) {
  const card = document.createElement("div");
  card.className = "ac-error";

  const head = document.createElement("div");
  head.className = "head";
  head.textContent = "✕ ERROR";

  const msg = document.createElement("div");
  msg.className = "msg";
  msg.textContent = message;

  const actions = document.createElement("div");
  actions.className = "actions";

  const retryBtn = document.createElement("button");
  retryBtn.type = "button";
  retryBtn.className = "ac-btn primary";
  retryBtn.textContent = "Повторить";
  retryBtn.addEventListener("click", onRetry);

  const newBtn = document.createElement("button");
  newBtn.type = "button";
  newBtn.className = "ac-btn ghost";
  newBtn.textContent = "Новая сессия";
  newBtn.addEventListener("click", onNewSession);

  actions.appendChild(retryBtn);
  actions.appendChild(newBtn);
  card.appendChild(head);
  card.appendChild(msg);
  card.appendChild(actions);
  log.appendChild(card);
  log.scrollTop = log.scrollHeight;
}

async function sendMessage(text) {
  lastUserMessage = text;
  messageCount++;
  refreshSessionInfo();
  input.disabled = true;
  sendButton.disabled = true;
  const { row, label } = appendStatus("claude думает…");
  activeStatusLabel = label;

  try {
    const reply = await invoke("send_chat_message", { message: text });
    row.remove();
    messageCount++;
    currentSessionId = reply.session_id ?? currentSessionId;
    appendAgent(reply.text);
    refreshSessionInfo();
  } catch (err) {
    row.remove();
    appendError(String(err), {
      onRetry: () => sendMessage(lastUserMessage),
      onNewSession: () => resetSession(),
    });
  } finally {
    activeStatusLabel = null;
    input.disabled = false;
    sendButton.disabled = false;
    input.focus();
  }
}

async function resetSession() {
  try {
    await invoke("reset_chat_session");
  } catch (err) {
    console.error("reset_chat_session failed", err);
  }
  log.replaceChildren();
  messageCount = 0;
  currentSessionId = null;
  refreshSessionInfo();
  appendAgentProse("Новая сессия. Каждое сообщение — реальный вызов Claude Code (платный).");
}

newSessionBtn.addEventListener("click", resetSession);

form.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = input.value.trim();
  if (!text) return;
  input.value = "";
  appendUser(text);
  sendMessage(text);
});

refreshSessionInfo();
input.focus();
console.log("✅ Chat JS loaded (agent-chat redesign)");
