/* Fixture-owned page code only. No title transport, eval, provider, or host IPC. */
"use strict";
(() => {
  const cfg = JSON.parse(document.getElementById("court-config").textContent);
  const input = document.getElementById("unsaved");
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  const instance = Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
  const cookie = () => {
    const prefix = cfg.storage_key + "=";
    const item = document.cookie.split(";").map(x => x.trim()).find(x => x.startsWith(prefix));
    return item ? decodeURIComponent(item.slice(prefix.length)) : null;
  };
  let initialCookie = null, initialStorage = null, error = null;
  try {
    initialCookie = cookie();
    initialStorage = localStorage.getItem(cfg.storage_key);
    document.cookie = cfg.storage_key + "=" + encodeURIComponent(cfg.write_value) + "; Path=/; Max-Age=600; SameSite=Strict";
    localStorage.setItem(cfg.storage_key, cfg.write_value);
  } catch (_) {
    error = "storage-unavailable";
  }
  input.value = "unsaved:" + instance;
  let seq = 0, challenge = null, stopped = false, inFlight = false;
  if (cfg.phase === "ordinary") {
    history.replaceState({courtInstance: instance, step: 1}, "", location.pathname);
    history.pushState({courtInstance: instance, step: 2}, "", location.pathname + "?step=two");
  }
  const endpoint = kind => `/${kind}/${cfg.run}/${cfg.phase}/${cfg.ticket}`;
  async function report(event) {
    if (stopped || seq >= 128) return;
    const state = history.state;
    let currentCookie = null, currentStorage = null;
    try {
      currentCookie = cookie();
      currentStorage = localStorage.getItem(cfg.storage_key);
    } catch (_) { error = "storage-unavailable"; }
    const body = {
      version: 1, run: cfg.run, phase: cfg.phase, ticket: cfg.ticket,
      instance, seq: ++seq, event, challenge, error,
      input: input.value, initial_cookie: initialCookie, initial_storage: initialStorage,
      cookie: currentCookie, storage: currentStorage,
      history_length: history.length,
      history_instance: state && state.courtInstance || null,
      history_step: state && state.step || null,
      path: location.pathname, query: location.search
    };
    try {
      const response = await fetch(endpoint("report"), {
        method: "POST", mode: "same-origin", credentials: "same-origin",
        cache: "no-store", headers: {"Content-Type": "application/json"},
        body: JSON.stringify(body)
      });
      if (!response.ok) stopped = true;
    } catch (_) { stopped = true; }
  }
  addEventListener("popstate", () => { void report("history"); });
  async function poll() {
    if (stopped || inFlight) return;
    inFlight = true;
    try {
      const response = await fetch(endpoint("command"), {
        credentials: "same-origin", mode: "same-origin", cache: "no-store"
      });
      if (!response.ok) { stopped = true; return; }
      const command = await response.json();
      if (command.op === "stop") { stopped = true; return; }
      if (command.op === "report") {
        challenge = command.challenge;
        await report("requested");
      } else if (command.op === "history-back") {
        challenge = command.challenge;
        history.back();
      } else if (command.op === "history-forward") {
        challenge = command.challenge;
        history.forward();
      } else if (command.op !== "wait") {
        stopped = true;
      }
    } catch (_) { stopped = true; }
    finally { inFlight = false; }
  }
  void report("loaded").then(() => {
    const timer = setInterval(() => {
      if (stopped) clearInterval(timer);
      else void poll();
    }, 200);
  });
})();
