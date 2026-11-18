// Switch Library frontend. Plain ES module, no build step.
import { initTorBox, setTorBoxVisible } from "./torbox.js";
import { invoke, listen, openUrl } from "./runtime.js";

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

const PAGE = 96;
const NEW_DAYS = 30;
const ICON = {
  check: '<svg viewBox="0 0 24 24"><path d="M20 6 9 17l-5-5"/></svg>',
  owned: '<svg viewBox="0 0 24 24"><path d="M4 7.5 12 3l8 4.5v9L12 21l-8-4.5z"/><path d="m4 7.5 8 4.5 8-4.5M12 12v9"/></svg>',
  star: '<svg viewBox="0 0 24 24"><path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2L12 17.3 6.4 20.2l1.1-6.2L3 9.6l6.2-.9z"/></svg>',
};

const state = {
  games: [],
  view: [],
  rendered: 0,
  filter: "all",
  sort: prefs("sort", "newest"),
  region: prefs("region", "US.en"),
  hideDemos: prefs("hideDemos", true),
  query: "",
  owned: new Set(prefs("owned", [])),
  wishlist: new Set(prefs("wishlist", [])),
  detail: null,
  shots: [],
  shotIndex: 0,
};

// ---------- persistence (per-user conveniences; failures are harmless) ----------
function prefs(key, fallback) {
  try {
    const v = localStorage.getItem(`sl:${key}`);
    return v === null ? fallback : JSON.parse(v);
  } catch { return fallback; }
}
function save(key, value) {
  try { localStorage.setItem(`sl:${key}`, JSON.stringify(value instanceof Set ? [...value] : value)); } catch {}
}

// ---------- formatting ----------
const escapeHtml = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
function parseDate(n) {
  if (!n) return null;
  const s = String(n);
  return new Date(Date.UTC(+s.slice(0, 4), +s.slice(4, 6) - 1, +s.slice(6, 8)));
}
const dateFmt = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric", timeZone: "UTC" });
const fmtDate = (n) => { const d = parseDate(n); return d ? dateFmt.format(d) : "—"; };
const fmtYear = (n) => (n ? String(n).slice(0, 4) : "");
function fmtSize(bytes) {
  if (!bytes) return "—";
  const units = ["B", "KB", "MB", "GB"];
  let i = 0, v = bytes;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
}
const initials = (name) => name.replace(/[^\p{L}\p{N} ]/gu, "").split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0]).join("").toUpperCase();
const todayNum = () => { const d = new Date(); return d.getFullYear() * 10000 + (d.getMonth() + 1) * 100 + d.getDate(); };
const isUpcoming = (n) => n && n > todayNum();
const isNew = (n) => { const d = parseDate(n); return d && Date.now() - d.getTime() < NEW_DAYS * 864e5 && d.getTime() <= Date.now(); };

// ---------- data ----------
async function loadCatalog(force = false) {
  const refresh = $("#refresh");
  refresh.classList.add("spinning");
  refresh.disabled = true;
  showProgress(true, "Loading catalog…");
  if (!state.games.length) renderSkeleton();
  try {
    const cat = await invoke("fetch_catalog", { region: state.region, force });
    state.games = cat.games.map((g) => ({ ...g, hay: `${g.name} ${g.publisher ?? ""} ${g.developer ?? ""} ${g.id}`.toLowerCase() }));
    state.fetchedAt = cat.fetchedAt;
    if (force && cat.fromCache) toast("Couldn't reach the catalog — showing the saved copy", true);
    else if (force) toast(`Catalog updated · ${cat.games.length.toLocaleString()} games`);
    applyView();
  } catch (err) {
    state.games = [];
    applyView();
    showEmpty("Couldn't load the catalog", String(err));
  } finally {
    refresh.classList.remove("spinning");
    refresh.disabled = false;
    showProgress(false);
  }
}

function showProgress(on, label = "") {
  const el = $("#progress");
  el.hidden = !on;
  el.classList.toggle("indeterminate", true);
  $(".progress-label", el).textContent = label;
}

function onProgress({ phase, received, total }) {
  const el = $("#progress");
  if (phase === "parse") {
    el.classList.add("indeterminate");
    $(".progress-label", el).textContent = "Processing catalog…";
    return;
  }
  const mb = (received / 1048576).toFixed(1);
  if (total) {
    el.classList.remove("indeterminate");
    $(".progress-fill", el).style.width = `${Math.min(100, (received / total) * 100)}%`;
    $(".progress-label", el).textContent = `Downloading catalog · ${mb} / ${(total / 1048576).toFixed(1)} MB`;
  } else {
    $(".progress-label", el).textContent = `Downloading catalog · ${mb} MB`;
  }
}

// ---------- filtering & sorting ----------
const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });
const SORTS = {
  // Released titles first (latest on top); upcoming ones follow, soonest first.
  newest: (a, b) => {
    const ua = isUpcoming(a.releaseDate), ub = isUpcoming(b.releaseDate);
    if (ua !== ub) return ua ? 1 : -1;
    const d = ua ? (a.releaseDate - b.releaseDate) : (b.releaseDate ?? 0) - (a.releaseDate ?? 0);
    return d || collator.compare(a.name, b.name);
  },
  oldest: (a, b) => (a.releaseDate ?? 99999999) - (b.releaseDate ?? 99999999) || collator.compare(a.name, b.name),
  title: (a, b) => collator.compare(a.name, b.name),
  publisher: (a, b) => collator.compare(a.publisher ?? "￿", b.publisher ?? "￿") || collator.compare(a.name, b.name),
};

function applyView() {
  const terms = state.query.toLowerCase().split(/\s+/).filter(Boolean);
  const base = state.games.filter((g) => !(state.hideDemos && g.demo));
  let view = base;
  if (state.filter === "owned") view = view.filter((g) => state.owned.has(g.id));
  if (state.filter === "wishlist") view = view.filter((g) => state.wishlist.has(g.id));
  if (terms.length) view = view.filter((g) => terms.every((t) => g.hay.includes(t)));
  view = [...view].sort(SORTS[state.sort]);
  state.view = view;

  $('[data-count="all"]').textContent = base.length.toLocaleString();
  $('[data-count="owned"]').textContent = state.owned.size.toLocaleString();
  $('[data-count="wishlist"]').textContent = state.wishlist.size.toLocaleString();

  const updated = state.fetchedAt ? ` · catalog updated ${new Date(state.fetchedAt * 1000).toLocaleString()}` : "";
  $("#summary").textContent = state.games.length ? `${view.length.toLocaleString()} of ${base.length.toLocaleString()} games${updated}` : "";

  $("#grid").replaceChildren();
  state.rendered = 0;
  if (!state.games.length) return;
  if (!view.length) {
    const msg = {
      owned: ["Nothing marked as owned yet", "Open a game and choose “Owned”, or use the box icon on a card."],
      wishlist: ["Your wishlist is empty", "Use the star on a card to keep track of games you want."],
    }[state.filter];
    showEmpty(...(terms.length || !msg ? ["No matches", `Nothing matches “${state.query}”. Try a shorter search or a title ID.`] : msg));
    return;
  }
  $("#empty").hidden = true;
  renderMore();
}

// ---------- rendering ----------
function cardHtml(g) {
  const owned = state.owned.has(g.id), wish = state.wishlist.has(g.id);
  const flags = [
    owned && '<span class="flag owned">Owned</span>',
    !owned && wish && '<span class="flag wishlist">Wishlist</span>',
    isUpcoming(g.releaseDate) && '<span class="flag soon">Upcoming</span>',
    isNew(g.releaseDate) && '<span class="flag new">New</span>',
  ].filter(Boolean).join("");
  const sub = [g.publisher, fmtYear(g.releaseDate)].filter(Boolean).join(" · ");
  return `
    <article class="card" data-id="${g.id}" tabindex="0" aria-label="${escapeHtml(g.name)}">
      <div class="card-art">
        <span class="fallback" aria-hidden="true">${escapeHtml(initials(g.name))}</span>
        ${g.iconUrl ? `<img src="${escapeHtml(g.iconUrl)}" alt="" loading="lazy" decoding="async">` : ""}
        <div class="card-flags">${flags}</div>
        <div class="card-quick">
          <button class="quick ${owned ? "on" : ""}" data-toggle="owned" title="${owned ? "Remove from owned" : "Mark as owned"}">${ICON.owned}</button>
          <button class="quick ${wish ? "on" : ""}" data-toggle="wishlist" title="${wish ? "Remove from wishlist" : "Add to wishlist"}">${ICON.star}</button>
        </div>
      </div>
      <div class="card-meta">
        <div class="card-title">${escapeHtml(g.name)}</div>
        <div class="card-sub">${escapeHtml(sub) || "&nbsp;"}</div>
      </div>
    </article>`;
}

function renderMore() {
  const slice = state.view.slice(state.rendered, state.rendered + PAGE);
  if (!slice.length) return;
  $("#grid").insertAdjacentHTML("beforeend", slice.map(cardHtml).join(""));
  state.rendered += slice.length;
}

function renderSkeleton() {
  $("#empty").hidden = true;
  $("#grid").innerHTML = Array.from({ length: 24 }, () => `
    <div class="card skeleton"><div class="card-art"></div>
      <div class="card-meta"><div class="line"></div><div class="line short"></div></div></div>`).join("");
}

function showEmpty(title, text) {
  const el = $("#empty");
  $("h2", el).textContent = title;
  $("p", el).textContent = text;
  el.hidden = false;
  $("#grid").replaceChildren();
}

function refreshCard(id) {
  const el = $(`.card[data-id="${id}"]`);
  const g = state.games.find((x) => x.id === id);
  if (el && g) el.outerHTML = cardHtml(g);
}

// ---------- collection ----------
function toggle(kind, id) {
  const set = state[kind];
  const on = !set.has(id);
  on ? set.add(id) : set.delete(id);
  if (kind === "owned" && on) state.wishlist.delete(id); // owning a game takes it off the wishlist
  save("owned", state.owned);
  save("wishlist", state.wishlist);
  if (state.filter !== "all") applyView();
  else {
    refreshCard(id);
    $(`[data-count="owned"]`).textContent = state.owned.size.toLocaleString();
    $(`[data-count="wishlist"]`).textContent = state.wishlist.size.toLocaleString();
  }
  if (state.detail?.id === id) renderDetailActions(state.detail);
}

// ---------- detail ----------
function openDetail(id) {
  const g = state.games.find((x) => x.id === id);
  if (!g) return;
  state.detail = g;
  const dlg = $("#detail");
  const banner = $(".detail-banner", dlg);
  banner.src = g.bannerUrl || g.screenshots[0] || g.iconUrl || "";
  banner.hidden = !banner.src;
  $(".detail-icon", dlg).src = g.iconUrl || "";
  $("#detailTitle").textContent = g.name;
  $(".detail-sub", dlg).textContent = [g.publisher, g.developer && g.developer !== g.publisher ? g.developer : null].filter(Boolean).join(" · ") || "Unknown publisher";
  renderDetailActions(g);

  const facts = [
    ["Release date", fmtDate(g.releaseDate)],
    ["Title ID", `<span class="mono" data-copy="${g.id}" title="Click to copy">${g.id}</span>`, true],
    ["Players", g.players ? `Up to ${g.players}` : "—"],
    ["Size", fmtSize(g.size)],
    ["Languages", g.languages.length ? g.languages.map((l) => l.toUpperCase()).join(", ") : "—", false, true],
  ];
  $(".facts", dlg).innerHTML = facts.map(([k, v, raw, wide]) => `<div${wide ? ' class="wide"' : ""}><dt>${k}</dt><dd>${raw ? v : escapeHtml(v)}</dd></div>`).join("");
  $(".chips", dlg).innerHTML = [...new Set([...g.category, ...g.ratingContent])].map((c) => `<span class="chip">${escapeHtml(c)}</span>`).join("");
  state.shots = g.screenshots;
  $(".shots", dlg).innerHTML = g.screenshots.map((s, i) => `<img src="${escapeHtml(s)}" data-shot="${i}" alt="Screenshot ${i + 1}" loading="lazy">`).join("");
  const paras = (g.description || "No description available.").split(/\n{1,}/).filter((p) => p.trim());
  $(".description", dlg).innerHTML = (g.intro ? `<p class="intro">${escapeHtml(g.intro)}</p>` : "") + paras.map((p) => `<p>${escapeHtml(p)}</p>`).join("");

  if (!dlg.open) dlg.showModal();
  dlg.scrollTop = 0;
}

function renderDetailActions(g) {
  const owned = state.owned.has(g.id), wish = state.wishlist.has(g.id);
  const ob = $('#detail [data-toggle="owned"]'), wb = $('#detail [data-toggle="wishlist"]');
  ob.classList.toggle("on", owned);
  ob.innerHTML = `${owned ? ICON.check : ICON.owned} ${owned ? "Owned" : "Mark as owned"}`;
  wb.classList.toggle("on", wish);
  wb.innerHTML = `${ICON.star} ${wish ? "On wishlist" : "Add to wishlist"}`;
}

// ---------- lightbox ----------
function openShot(i) {
  if (!state.shots.length) return;
  state.shotIndex = (i + state.shots.length) % state.shots.length;
  const lb = $("#lightbox");
  $("img", lb).src = state.shots[state.shotIndex];
  $(".lb-count", lb).textContent = `${state.shotIndex + 1} / ${state.shots.length}`;
  lb.hidden = false;
}
const closeShot = () => { $("#lightbox").hidden = true; };

// Save the current screenshot. Falls back to opening it in a browser tab when the
// image can't be fetched directly (e.g. the host doesn't allow cross-origin reads).
async function saveShot() {
  const src = state.shots[state.shotIndex];
  if (!src) return;
  const name = decodeURIComponent(src.split("/").pop().split("?")[0]) || "screenshot.jpg";
  try {
    const res = await fetch(src);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const url = URL.createObjectURL(await res.blob());
    const a = Object.assign(document.createElement("a"), { href: url, download: name });
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  } catch {
    openUrl(src);
  }
}

// ---------- misc ----------
function toast(text, error = false) {
  const el = document.createElement("div");
  el.className = `toast${error ? " error" : ""}`;
  el.textContent = text;
  $("#toasts").append(el);
  setTimeout(() => el.remove(), 2700);
}

async function copy(text) {
  try { await navigator.clipboard.writeText(text); toast(`Copied ${text}`); }
  catch { toast("Couldn't copy to clipboard", true); }
}

function openExternal(url) {
  openUrl(url);
}

// ---------- wiring ----------
async function init() {

  const regions = await invoke("regions").catch(() => ["US.en"]);
  if (!regions.includes(state.region)) state.region = regions[0];
  $("#region").innerHTML = regions.map((r) => `<option value="${r}">${r.split(".")[0]}</option>`).join("");
  $("#region").value = state.region;
  $("#sort").value = state.sort;
  $("#hideDemos").checked = state.hideDemos;

  listen("catalog-progress", (e) => onProgress(e.payload));

  // Top-level sections: the game library and the TorBox manager.
  const showView = (view) => {
    $$(".views [data-view]").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.view === view)));
    $("#libraryView").hidden = view !== "library";
    $("#torboxView").hidden = view !== "torbox";
    document.body.dataset.view = view;
    setTorBoxVisible(view === "torbox");
    save("view", view);
  };
  $$(".views [data-view]").forEach((b) => b.addEventListener("click", () => showView(b.dataset.view)));
  initTorBox({ toast });
  showView(prefs("view", "library"));

  let searchTimer;
  const search = $("#search"), searchClear = $("#searchClear");
  const syncClear = () => { searchClear.hidden = !search.value; };
  const clearSearch = () => {
    clearTimeout(searchTimer);
    search.value = "";
    syncClear();
    state.query = "";
    applyView();
    search.focus();
  };
  search.addEventListener("input", (e) => {
    syncClear();
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => { state.query = e.target.value.trim(); applyView(); }, 120);
  });
  search.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && search.value) { e.preventDefault(); clearSearch(); }
  });
  searchClear.addEventListener("click", clearSearch);
  $("#sort").addEventListener("change", (e) => { state.sort = e.target.value; save("sort", state.sort); applyView(); });
  $("#hideDemos").addEventListener("change", (e) => { state.hideDemos = e.target.checked; save("hideDemos", state.hideDemos); applyView(); });
  $("#region").addEventListener("change", (e) => { state.region = e.target.value; save("region", state.region); state.games = []; loadCatalog(); });
  $("#refresh").addEventListener("click", () => loadCatalog(true));
  $$(".segmented [data-filter]").forEach((b) => b.addEventListener("click", () => {
    state.filter = b.dataset.filter;
    $$(".segmented [data-filter]").forEach((x) => x.setAttribute("aria-selected", String(x === b)));
    applyView();
  }));

  const grid = $("#grid");
  grid.addEventListener("click", (e) => {
    const card = e.target.closest(".card[data-id]");
    if (!card) return;
    const t = e.target.closest("[data-toggle]");
    if (t) { e.stopPropagation(); toggle(t.dataset.toggle, card.dataset.id); return; }
    openDetail(card.dataset.id);
  });
  grid.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target.matches(".card[data-id]")) openDetail(e.target.dataset.id);
  });
  grid.addEventListener("load", (e) => { if (e.target.tagName === "IMG") e.target.classList.add("loaded"); }, true);
  grid.addEventListener("error", (e) => { if (e.target.tagName === "IMG") e.target.remove(); }, true);

  new IntersectionObserver((entries) => {
    if (entries.some((x) => x.isIntersecting)) renderMore();
  }, { rootMargin: "1200px" }).observe($("#sentinel"));

  const dlg = $("#detail");
  dlg.addEventListener("click", (e) => {
    if (e.target === dlg || e.target.closest("[data-close]")) return dlg.close();
    const t = e.target.closest("[data-toggle]");
    if (t) return toggle(t.dataset.toggle, state.detail.id);
    if (e.target.closest("[data-eshop]")) return openExternal(`https://www.nintendo.com/us/search/#q=${encodeURIComponent(state.detail.name)}`);
    const c = e.target.closest("[data-copy]");
    if (c) return copy(c.dataset.copy);
    const s = e.target.closest("[data-shot]");
    if (s) return openShot(+s.dataset.shot);
  });

  const lb = $("#lightbox");
  lb.addEventListener("click", (e) => {
    if (e.target.closest(".prev")) return openShot(state.shotIndex - 1);
    if (e.target.closest(".next")) return openShot(state.shotIndex + 1);
    if (e.target.closest(".lb-save")) return saveShot();
    closeShot();
  });

  document.addEventListener("keydown", (e) => {
    if (!lb.hidden) {
      if (e.key === "Escape") { e.preventDefault(); closeShot(); }
      if (e.key === "ArrowLeft") openShot(state.shotIndex - 1);
      if (e.key === "ArrowRight") openShot(state.shotIndex + 1);
      return;
    }
    if (e.key === "/" && document.body.dataset.view !== "torbox" && document.activeElement?.tagName !== "INPUT" && !dlg.open) {
      e.preventDefault();
      $("#search").focus();
    }
  });

  loadCatalog();
}

init();
