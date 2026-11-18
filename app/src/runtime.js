// Thin client for the local Switch Library server.
// invoke(cmd, args) -> POST /api/<cmd>; listen(event, fn) -> Server-Sent Events.

export async function invoke(cmd, args = {}) {
  let res;
  try {
    res = await fetch(`/api/${cmd}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Switch-Library": "1" },
      body: JSON.stringify(args),
    });
  } catch {
    throw "Can't reach the Switch Library server. Is it still running?";
  }
  const body = await res.json().catch(() => null);
  if (!res.ok) throw body?.error ?? `Server error ${res.status}`;
  return body;
}

let source;
const handlers = new Map();
export function listen(event, fn) {
  source ??= new EventSource("/api/events");
  if (!handlers.has(event)) {
    handlers.set(event, []);
    source.addEventListener(event, (e) => {
      let payload;
      try { payload = JSON.parse(e.data); } catch { return; }
      handlers.get(event).forEach((h) => h({ payload }));
    });
  }
  handlers.get(event).push(fn);
}

export function openUrl(url) {
  window.open(url, "_blank", "noopener,noreferrer");
}

export const revealItemInDir = (path) => invoke("reveal", { path });
