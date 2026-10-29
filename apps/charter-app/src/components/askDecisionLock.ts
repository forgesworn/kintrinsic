import { useSyncExternalStore } from "react";

// One ask decision in flight at a time, across every card on the screen.
//
// The old Approvals screen held a single `busy` for all its asks. With cards
// now spread over Home and the child page, each keeping its own, two quick
// taps on different asks for one child could sign two `apps` clauses from the
// same pre-state, and the later one would drop the earlier hold. So the lock
// lives here, outside any one card: every answer button reads it, and a
// decision only starts if it can take it. Module-level (not tied to a mount),
// so a card that unmounts mid-send still releases it when the send settles.

/** How long a decision may hold the lock. A remote signer can sit on a request
 *  with no answer; past this the lock is treated as free rather than leaving
 *  every answer button dead until the page is reloaded. */
export const ASK_DECISION_STALE_MS = 60_000;

interface Hold {
  token: symbol;
  since: number;
  timer: ReturnType<typeof setTimeout>;
}

let holder: Hold | null = null;
const listeners = new Set<() => void>();

const emit = () => {
  for (const l of listeners) l();
};

const isHeld = () => holder !== null && Date.now() - holder.since < ASK_DECISION_STALE_MS;

function clear() {
  if (holder) clearTimeout(holder.timer);
  holder = null;
}

/**
 * Take the lock for a decision. Returns the token to release it with, or null
 * (and nothing changes) if another decision is in flight. A hold older than
 * `ASK_DECISION_STALE_MS` doesn't count: it is replaced, and the stale
 * decision's own `endAskDecision` then does nothing.
 */
export function tryBeginAskDecision(): symbol | null {
  if (isHeld()) return null;
  clear();
  const token = Symbol("ask-decision");
  // Wakes the hook at expiry so the buttons come back without another render.
  const timer = setTimeout(() => {
    if (holder?.token === token) {
      clear();
      emit();
    }
  }, ASK_DECISION_STALE_MS);
  holder = { token, since: Date.now(), timer };
  emit();
  return token;
}

/** Release the lock, if `token` still holds it. */
export function endAskDecision(token: symbol): void {
  if (holder?.token !== token) return;
  clear();
  emit();
}

/** True while ANY ask decision is being sent. */
export function useAskDecisionInFlight(): boolean {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    isHeld,
    () => false,
  );
}

/** Drop the lock whatever holds it. For tests, which can leave a send pending. */
export function resetAskDecisionLock(): void {
  clear();
  emit();
}
