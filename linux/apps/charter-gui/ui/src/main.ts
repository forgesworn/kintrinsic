// Headless-core entry. The on-display Tauri UX is deferred (no WebKitGTK here);
// this just wires the lock + countdown models so `vite build` has an entry.

import { Countdown } from "./countdown";
import { LockState } from "./lock";
import { CONTRACT } from "./ipc";

export function boot(): { lock: LockState; countdown: Countdown } {
  const lock = new LockState();
  const countdown = new Countdown();
  const el = document.getElementById("app");
  if (el) {
    el.textContent = `Charter (${CONTRACT.interface})`;
  }
  return { lock, countdown };
}

if (typeof document !== "undefined" && document.getElementById("app")) {
  boot();
}
