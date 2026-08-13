// The ORDER `approveAppOpen` runs its two signed steps in, extracted so the
// sequencing itself — not a hand-simulation of it — is what a test exercises.
//
// N2 (review round 2, 2026-08-03): the CLAUSE (the `AppHold` that actually
// opens the app) must be signed and confirmed SENT before the plain Decision
// echo (the courtesy "closes the ward's UI loop" signal) is ever published.
// The original order was the reverse — echo first, clause second — which let
// a declined clause-sign confirm (autoSign off is the DEFAULT; the guardian
// taps "cancel" on the SECOND of two prompts) slip through `savePolicy`'s own
// silent `{outcome:"cancelled"}` return: the request still got marked
// approved and the activity log still said "You let them open Minecraft"
// while no hold ever landed and the app stayed blocked — the ward's card
// would have read "You can open it!" over a lie.

import type { ClauseDelivery } from "./clauseDelivery";

export interface ApproveAppOpenFlowDeps {
  /** Sign + save the `apps` clause carrying the `AppHold`. */
  saveHold: () => Promise<ClauseDelivery>;
  /** Sign + publish the plain Decision echo. Resolves `false` only when the
   *  confirm was declined (never claim minutes travelled when nothing was
   *  signed). */
  sendDecision: () => Promise<boolean>;
}

export type ApproveAppOpenOutcome =
  | { outcome: "approved" }
  /** The Decision echo's own confirm was declined — the hold DID land (the
   *  app opens), only the courtesy "closes the loop" signal didn't send. Not
   *  an error: nothing to refuse, nothing false was claimed either. */
  | { outcome: "decision-cancelled" }
  /** The clause never reached the wire — refuse the WHOLE approval. Neither
   *  step is allowed to proceed past this. */
  | { outcome: "refused"; message: string };

/** Human copy for a clause that didn't send, by why. */
function refusalMessage(delivery: Extract<ClauseDelivery, { outcome: "cancelled" | "undelivered" }>): string {
  return delivery.outcome === "cancelled"
    ? "Nothing was changed — the hold wasn't signed."
    : "Can't reach their device right now, so nothing was changed. Turn on parent approval, then try again.";
}

/**
 * Run the two signed steps of approving an `app.open` ask, IN THE CORRECT
 * ORDER: the clause first, and the Decision echo ONLY once the clause is
 * confirmed `"sent"`. The caller (`store.tsx`'s `approveAppOpen`) is
 * responsible for everything this function has no business doing — building
 * the hold, and (on `"approved"` only) marking the request approved and
 * logging the activity entry.
 */
export async function runApproveAppOpenFlow(
  deps: ApproveAppOpenFlowDeps,
): Promise<ApproveAppOpenOutcome> {
  const delivery = await deps.saveHold();
  if (delivery.outcome !== "sent") {
    return { outcome: "refused", message: refusalMessage(delivery) };
  }
  const ok = await deps.sendDecision();
  if (!ok) return { outcome: "decision-cancelled" };
  return { outcome: "approved" };
}
