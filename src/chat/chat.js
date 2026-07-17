const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;

const currentWin = getCurrentWindow();

document.getElementById("close-btn").addEventListener("click", () => {
  currentWin.hide();
});

const log = document.getElementById("log");
const form = document.getElementById("composer");
const input = document.getElementById("input");
const sendButton = form.querySelector('button[type="submit"]');

function appendMessage(role, text) {
  const row = document.createElement("div");
  row.className = `msg msg-${role}`;
  const bubble = document.createElement("span");
  bubble.className = "bubble";
  bubble.textContent = text;
  row.appendChild(bubble);
  log.appendChild(row);
  log.scrollTop = log.scrollHeight;
  return row;
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const text = input.value.trim();
  if (!text) return;

  appendMessage("user", text);
  input.value = "";
  input.disabled = true;
  sendButton.disabled = true;
  const pending = appendMessage("assistant", "…");

  try {
    const reply = await invoke("send_chat_message", { message: text });
    pending.querySelector(".bubble").textContent = reply;
  } catch (err) {
    pending.remove();
    appendMessage("assistant", `Ошибка: ${err}`);
  } finally {
    input.disabled = false;
    sendButton.disabled = false;
    input.focus();
  }
});

input.focus();
console.log("✅ Chat JS loaded");
