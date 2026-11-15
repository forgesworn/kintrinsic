// The home-network link grace after the guardian disconnects a ward's phone.
//
// Why: the release goes out over the wire the moment the guardian disconnects
// a device, and the phone answers it with an acknowledgement and every audit it
// was holding (the contract's "A guardian's release and the held audits"). If
// the release went over the home network, that answer comes back over the same
// local link, which the carrier closes once the machine is no longer paired.
// So the roster entry for a released machine carries `linkUntil` (epoch ms),
// and the carrier admits a released machine's link until that time. The
// carrier enforces the deadline itself (a timer of its own, and a check at the
// handshake), so it ends even when this page is backgrounded or killed.
//
// The deadline: from the click it is 2 minutes (a slow approval on the confirm
// sheet must not cost the grace). When the release send settles, resolved or
// failed, it becomes that moment plus 10 seconds, and the roster is pushed
// again so the carrier hears it.
//
// This is a roster-push concern only. It is held in memory, never persisted
// (a reload simply has no grace; the carrier keeps what it was last sent), and
// the entry is ALWAYS sent as released, never as paired: nothing in the console
// or the carrier treats the device as paired because of it.

/** How long after the release send settles the carrier keeps the machine's link. */
export const RELEASE_LINK_GRACE_MS = 10_000;

/** How long the link is kept from the click, while the release is still being sent. */
export const RELEASE_SEND_GRACE_MS = 2 * 60_000;

const until = new Map<string, number>();
/** Which release each deadline belongs to, so a late answer to an earlier one cannot move a newer one. */
const tokens = new Map<string, number>();
let nextToken = 0;
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

/**
 * The guardian has just disconnected this machine and the release is about to
 * be sent: the link is kept for 2 minutes from now. Returns this release's
 * token, to hand back to `settleReleaseGrace`.
 */
export function startReleaseGrace(machine: string, now: number = Date.now()): number {
  const old = timers.get(machine);
  if (old !== undefined) clearTimeout(old);
  const token = ++nextToken;
  tokens.set(machine, token);
  until.set(machine, now + RELEASE_SEND_GRACE_MS);
  arm(machine);
  return token;
}

/**
 * The release send has settled, resolved or failed: the link is kept for 10
 * more seconds from now, and the listeners are told so the roster is pushed
 * with the new deadline. Ignored when a newer release to the same machine has
 * started since, or when the grace has already run out.
 */
export function settleReleaseGrace(machine: string, token: number, now: number = Date.now()): void {
  if (tokens.get(machine) !== token) return;
  const deadline = until.get(machine);
  // Already over (the carrier let it go on its own deadline): never bring it back.
  if (deadline === undefined || now >= deadline) return;
  const old = timers.get(machine);
  if (old !== undefined) clearTimeout(old);
  until.set(machine, now + RELEASE_LINK_GRACE_MS);
  arm(machine);
  notify();
}

/**
 * The deadlines still running at `now`, by machine: what the roster's
 * `linkUntil` carries. A machine with no grace, or one that has run out, is not
 * in it.
 */
export function releaseLinkDeadlines(now: number = Date.now()): Map<string, number> {
  const out = new Map<string, number>();
  for (const [machine, deadline] of until) {
    if (now < deadline) out.set(machine, deadline);
  }
  return out;
}

/** Be told when a deadline has changed or run out (the roster must be pushed again then). */
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
  tokens.clear();
  listeners.clear();
}
