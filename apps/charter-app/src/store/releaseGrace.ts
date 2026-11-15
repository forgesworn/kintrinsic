// A short grace for the home-network link after the guardian disconnects a
// ward's phone.
//
// Why: the release goes out over the wire the moment the guardian disconnects
// a device, and the phone answers it with an acknowledgement and every audit it
// was holding (the contract's "A guardian's release and the held audits"). If
// the release went over the home network, that answer comes back over the same
// local link, which the carrier closes as soon as the roster stops listing the
// machine as paired. So the roster keeps listing it as paired for a few
// seconds after the release is sent, and then lets it go.
//
// This is a roster-push concern only. It is held in memory, never persisted
// (a reload simply has no grace, and the link is gone anyway), and nothing in
// the console treats the device as paired during it: the household and the
// persisted state say "unpaired" from the first moment.

/** How long after the release is sent the carrier keeps the machine's link. */
export const RELEASE_LINK_GRACE_MS = 10_000;

const until = new Map<string, number>();
const timers = new Map<string, ReturnType<typeof setTimeout>>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const fn of [...listeners]) {
    try {
      fn();
    } catch {
      /* a listener's failure must not stop the others */
    }
  }
}

function arm(machine: string): void {
  const deadline = until.get(machine);
  if (deadline === undefined) return;
  const timer = setTimeout(() => {
    timers.delete(machine);
    const d = until.get(machine);
    if (d === undefined) return;
    if (Date.now() < d) {
      // A timer can fire a hair early; wait out the remainder.
      arm(machine);
      return;
    }
    until.delete(machine);
    notify();
  }, Math.max(0, deadline - Date.now()));
  timers.set(machine, timer);
}

/** The release to this machine has just been sent: start (or restart) its grace. */
export function startReleaseGrace(machine: string, now: number = Date.now()): void {
  const old = timers.get(machine);
  if (old !== undefined) clearTimeout(old);
  until.set(machine, now + RELEASE_LINK_GRACE_MS);
  arm(machine);
}

/**
 * Drop the grace now (the release never went out, so there is nothing to wait
 * for), and tell the listeners so the roster is pushed again.
 */
export function endReleaseGrace(machine: string): void {
  const timer = timers.get(machine);
  if (timer !== undefined) clearTimeout(timer);
  timers.delete(machine);
  if (until.delete(machine)) notify();
}

/** The machines still inside their grace at `now`. */
export function releasingMachines(now: number = Date.now()): Set<string> {
  const out = new Set<string>();
  for (const [machine, deadline] of until) {
    if (now < deadline) out.add(machine);
  }
  return out;
}

/** Be told when a grace has ended (the roster must be pushed again then). */
export function onReleaseGraceEnd(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** Test seam: forget every grace and cancel its timer. */
export function resetReleaseGraceForTests(): void {
  for (const t of timers.values()) clearTimeout(t);
  timers.clear();
  until.clear();
  listeners.clear();
}
