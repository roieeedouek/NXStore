// TorBox manager tab: log in with an API key, add links, watch status, download.
const tauri = window.__TAURI__;
const invoke = (cmd, args) => tauri.core.invoke(cmd, args);
const $ = (sel, root = document) => root.querySelector(sel);

const FAST_POLL = 4000;   // while something is still processing on TorBox
const SLOW_POLL = 20000;  // when everything is idle

const tb = {
  account: null,
  transfers: [],
  local: new Map(),       // "torrents-12" -> { received, total, done, error, path }
  timer: null,
  visible: false,
  confirmRemove: null,
};

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const keyOf = (t) => `${t.kind === "torrent" ? "torrents" : "webdl"}-${t.id}`;

function bytes(n) {
  if (!n) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(u.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  const v = n / 1024 ** i;
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${u[i]}`;
}
function duration(s) {
  if (!s || s <= 0) return "";
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.round(s / 60)}m`;
  return `${Math.floor(s / 3600)}h ${Math.round((s % 3600) / 60)}m`;
}

const STATE_LABEL = {
  downloading: "Downloading",
  uploading: "Ready · seeding",
  completed: "Ready",
  cached: "Ready",
  paused: "Paused",
  stalled: "Stalled",
  "stalled (no seeds)": "Stalled · no seeds",
  metaDL: "Fetching metadata",
  checkingResumeData: "Checking",
  queued: "Queued",
  error: "Error",
  failed: "Failed",
};
function stateInfo(t) {
  if (t.ready) return { label: STATE_LABEL[t.state] ?? "Ready", tone: "ready" };
  const s = t.state ?? "";
  if (/error|fail/i.test(s)) return { label: STATE_LABEL[s] ?? s, tone: "error" };
  if (/stall|pause/i.test(s)) return { label: STATE_LABEL[s] ?? s, tone: "warn" };
  return { label: STATE_LABEL[s] ?? (s ? s[0].toUpperCase() + s.slice(1) : "Processing"), tone: "busy" };
}

// ---------- session ----------
function showLoggedIn(account) {
  tb.account = account;
  $("#tbLogin").hidden = true;
  $("#tbApp").hidden = false;
  $("#tbEmail").textContent = account.email || "TorBox account";
  const plan = [account.plan, account.premiumExpiresAt && `until ${new Date(account.premiumExpiresAt).toLocaleDateString()}`].filter(Boolean).join(" · ");
  $("#tbPlan").textContent = plan;
  $(".tb-avatar").textContent = (account.email || "T")[0].toUpperCase();
  refresh();
}

function showLoggedOut(error) {
  tb.account = null;
  tb.transfers = [];
  clearTimeout(tb.timer);
  $("#tbApp").hidden = true;
  $("#tbLogin").hidden = false;
  const err = $("#tbLoginError");
  err.hidden = !error;
  err.textContent = error || "";
}

async function login(e) {
  e.preventDefault();
  const btn = $("#tbLoginForm button[type=submit]");
  btn.disabled = true;
  btn.textContent = "Checking…";
  try {
    const account = await invoke("torbox_login", { apiKey: $("#tbKey").value });
    $("#tbKey").value = "";
    showLoggedIn(account);
    notify("Connected to TorBox");
  } catch (err) {
    showLoggedOut(String(err));
  } finally {
    btn.disabled = false;
    btn.textContent = "Log in";
  }
}

async function logout() {
  await invoke("torbox_logout").catch(() => {});
  showLoggedOut();
}

// ---------- transfers ----------
async function refresh() {
  clearTimeout(tb.timer);
  if (!tb.account) return;
  try {
    tb.transfers = await invoke("torbox_list");
    $("#tbUpdated").textContent = `Updated ${new Date().toLocaleTimeString()}`;
    render();
  } catch (err) {
    if (/rejected the API key|Not logged in/.test(String(err))) return showLoggedOut(String(err));
    $("#tbUpdated").textContent = String(err);
  }
  schedule();
}

function schedule() {
  clearTimeout(tb.timer);
  if (!tb.account || !tb.visible) return;
  const busy = tb.transfers.some((t) => !t.ready && !/error|fail/i.test(t.state));
  tb.timer = setTimeout(refresh, busy ? FAST_POLL : SLOW_POLL);
}

async function add(e) {
  e.preventDefault();
  const input = $("#tbLink");
  const link = input.value.trim();
  if (!link) return input.focus();
  const btn = $("#tbAddForm button");
  btn.disabled = true;
  btn.textContent = "Adding…";
  try {
    const res = await invoke("torbox_add", { link });
    input.value = "";
    notify(res.kind === "torrent" ? "Torrent added" : "Link added");
    await refresh();
  } catch (err) {
    notify(String(err), true);
  } finally {
    btn.disabled = false;
    btn.textContent = "Add";
  }
}

async function download(t) {
  const key = keyOf(t);
  tb.local.set(key, { received: 0, total: null, done: false });
  render();
  try {
    await invoke("torbox_download", { kind: t.kind, id: t.id, name: t.name, fileCount: t.fileCount });
  } catch (err) {
    // The backend also emits a final event with the error; this covers failures before streaming starts.
    const cur = tb.local.get(key);
    if (!cur?.done) { tb.local.set(key, { done: true, error: String(err) }); render(); }
  }
}

async function remove(t, btn) {
  const key = keyOf(t);
  if (tb.confirmRemove !== key) {
    tb.confirmRemove = key;
    btn.textContent = "Remove?";
    btn.classList.add("danger");
    setTimeout(() => { if (tb.confirmRemove === key) { tb.confirmRemove = null; render(); } }, 3000);
    return;
  }
  tb.confirmRemove = null;
  try {
    await invoke("torbox_remove", { kind: t.kind, id: t.id });
    tb.local.delete(key);
    notify("Removed from TorBox");
    refresh();
  } catch (err) {
    notify(String(err), true);
  }
}

function onDownloadEvent({ key, received, total, done, error, path }) {
  tb.local.set(key, { received, total, done, error, path });
  if (done && !error) notify("Download finished");
  if (done && error) notify(error, true);
  if (tb.visible) render();
}

// ---------- rendering ----------
function actionHtml(t) {
  const l = tb.local.get(keyOf(t));
  if (l && !l.done) {
    const pct = l.total ? Math.round((l.received / l.total) * 100) : null;
    return `<div class="tb-local"><span>${pct !== null ? `${pct}%` : bytes(l.received)}</span><span class="tb-local-sub">${l.total ? `${bytes(l.received)} of ${bytes(l.total)}` : "Saving…"}</span></div>`;
  }
  if (l?.done && l.path) {
    return `<button class="pill-btn" data-act="reveal">Show in folder</button>`;
  }
  if (t.ready) {
    return `<button class="primary-btn small" data-act="download">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v12m0 0-4-4m4 4 4-4"/><path d="M4 17v2a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-2"/></svg>
      ${l?.error ? "Retry" : "Download"}</button>`;
  }
  return "";
}

function rowHtml(t) {
  const key = keyOf(t);
  const s = stateInfo(t);
  const l = tb.local.get(key);
  const pct = Math.round(t.progress * 100);
  const localPct = l && !l.done && l.total ? (l.received / l.total) * 100 : null;
  const meta = [
    `<span class="tb-kind">${t.kind === "torrent" ? "Torrent" : "Web"}</span>`,
    `<span class="tb-state ${s.tone}">${esc(s.label)}</span>`,
    t.size ? `<span>${bytes(t.size)}</span>` : "",
    t.fileCount > 1 ? `<span>${t.fileCount} files</span>` : "",
    !t.ready && t.speed ? `<span>${bytes(t.speed)}/s</span>` : "",
    !t.ready && t.eta ? `<span>${duration(t.eta)} left</span>` : "",
  ].filter(Boolean).join("");
  return `
    <article class="tb-row ${s.tone}" data-key="${key}">
      <div class="tb-main">
        <div class="tb-name" title="${esc(t.name)}">${esc(t.name)}</div>
        <div class="tb-meta">${meta}</div>
        <div class="tb-bar ${localPct !== null ? "local" : ""}"><span style="width:${localPct ?? pct}%"></span></div>
        ${l?.error ? `<div class="tb-row-error">${esc(l.error)}</div>` : ""}
      </div>
      <div class="tb-actions">
        ${actionHtml(t)}
        <button class="icon-btn" data-act="remove" title="Remove from TorBox" aria-label="Remove from TorBox">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/></svg>
        </button>
      </div>
    </article>`;
}

function render() {
  const list = $("#tbList");
  list.innerHTML = tb.transfers.map(rowHtml).join("");
  $("#tbEmpty").hidden = tb.transfers.length > 0;
}

let notify = () => {};

// ---------- wiring ----------
export function initTorBox({ toast }) {
  notify = toast;
  $("#tbLoginForm").addEventListener("submit", login);
  $("#tbAddForm").addEventListener("submit", add);
  $("#tbLogout").addEventListener("click", logout);
  $("#tbKeyShow").addEventListener("click", (e) => {
    const k = $("#tbKey");
    k.type = k.type === "password" ? "text" : "password";
    e.target.textContent = k.type === "password" ? "Show" : "Hide";
  });
  $("#torboxView").addEventListener("click", (e) => {
    const ext = e.target.closest("[data-ext]");
    if (ext) return tauri.opener.openUrl(ext.dataset.ext).catch(() => {});
    const btn = e.target.closest("[data-act]");
    if (!btn) return;
    const t = tb.transfers.find((x) => keyOf(x) === btn.closest(".tb-row").dataset.key);
    if (!t) return;
    if (btn.dataset.act === "download") download(t);
    if (btn.dataset.act === "remove") remove(t, btn);
    if (btn.dataset.act === "reveal") tauri.opener.revealItemInDir(tb.local.get(keyOf(t)).path).catch((err) => notify(String(err), true));
  });
  tauri.event.listen("torbox-download", (e) => onDownloadEvent(e.payload));
  document.addEventListener("visibilitychange", () => (document.hidden ? clearTimeout(tb.timer) : tb.visible && refresh()));

  invoke("torbox_session")
    .then((acc) => (acc ? showLoggedIn(acc) : showLoggedOut()))
    .catch((err) => showLoggedOut(`Saved key no longer works: ${err}`));
}

export function setTorBoxVisible(on) {
  tb.visible = on;
  if (on) refresh();
  else clearTimeout(tb.timer);
}
