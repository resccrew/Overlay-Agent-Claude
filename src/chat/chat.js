// UI-only for now. Wiring this up to a real Claude Code session is a
// separate, not-yet-started task ("Подключение чат-попапа к Claude Code").
// Deliberately not faking a connection here.

const log = document.getElementById("log");
const form = document.getElementById("composer");
const input = document.getElementById("input");

function appendMessage(role, text) {
  const row = document.createElement("div");
  row.className = `msg msg-${role}`;
  const bubble = document.createElement("span");
  bubble.className = "bubble";
  bubble.textContent = text;
  row.appendChild(bubble);
  log.appendChild(row);
  log.scrollTop = log.scrollHeight;
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  const text = input.value.trim();
  if (!text) return;
  appendMessage("user", text);
  input.value = "";

  // Stub reply so the UI is visibly interactive without pretending to be
  // connected to anything real.
  setTimeout(() => {
    appendMessage("assistant", "(заглушка — реального подключения к Claude Code пока нет)");
  }, 200);
});

input.focus();
