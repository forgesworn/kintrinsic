// The minutes/tz/wire-correlation slice of a `DecisionContext` — split out
// of `store.tsx`'s `decisionContext` so the REAL composition, not a
// hand-built fixture, is what a test exercises.
//
// N1 (review round 2, 2026-08-03): `decisionContext`'s `minutesGranted`
// ternary only ever covered `"time.extend"`; every other kind — including
// `"app.open"` once it gained a real grant slot in review round 1 — fell to
// `undefined`, and `realSigner.ts`'s `signDecision` falls back to
// `ctx?.minutesGranted ?? 0` when that happens. The result: every ALLOWED
// app.open shipped `minutesGranted: 0` — the documented DENY value — in the
// very field the round added, allow and deny indistinguishable on the wire.
// `signer/realSigner.test.ts` hand-builds `DecisionContext` directly, so it
// could never have caught this (the "tests that grant what prod forgets"
// pattern — see the memory note of the same name). Extracting the real
// composition into this pure, exported function is what makes a test able to
// go THROUGH it instead of around it.

import type { AppOpenDecisionWire, DecisionWire } from "../signer/Signer";
import type { ChildRequest } from "../domain/types";
import { pickExtendTz } from "../wire/grant";

export interface DecisionTimingInput {
  req: ChildRequest;
  /** The parent's chosen minutes, when the caller has one. For `time.extend`
   *  this defaults to what the child asked for when omitted; `app.open` has
   *  no such fallback to reach for — the guardian's picked WINDOW duration
   *  is the only source, so an omission there must still resolve to `0`
   *  (a safe non-value), never `undefined` (which downstream silently reads
   *  as "deny" — see the file doc). */
  minutesGranted?: number;
  scheduleTz?: string;
  budgetTz?: string;
  /** The buckets clause's own tz — required on the wire, so always present
   *  when the child has ANY named-times group. A FALLBACK only: `pickExtendTz`
   *  resolves schedule → budget → buckets, so this is consulted exactly when
   *  the ward carries neither of the other two (the named-times-only ward of
   *  C-1). Never an override — a working schedule ward signs unchanged. */
  bucketsTz?: string;
}

export interface DecisionTiming {
  minutesGranted?: number;
  tz?: string;
  wire?: DecisionWire;
  appOpen?: AppOpenDecisionWire;
}

/** Build the minutes/tz/wire-correlation a decision's GRANT needs, for
 *  whichever kind `req` actually is. `install.apk`'s correlation is built
 *  separately in `store.tsx` (it needs the catalog + update-manifest lookups
 *  this function has no business depending on). */
export function decisionTiming({
  req,
  minutesGranted,
  scheduleTz,
  budgetTz,
  bucketsTz,
}: DecisionTimingInput): DecisionTiming {
  const wire: DecisionWire | undefined =
    req.kind === "time.extend" &&
    req.reqId && req.nonce && req.machine && req.subject && req.limitHit
      ? {
          reqId: req.reqId,
          nonce: req.nonce,
          machine: req.machine,
          subject: req.subject,
          limitHit: req.limitHit,
          // Named times: echoed verbatim into the GRANT, exactly like
          // limitHit itself — absent for the whole-device dimensions.
          bucketId: req.bucketId,
        }
      : undefined;
  // app.open: the plain Decision echo's wire correlation — {pkg,
  // minutesGranted}. The actual permission is a SEPARATE apps-clause save
  // (see `store.tsx`'s `approveAppOpen`), never this GRANT's enactable
  // effect.
  const appOpen: AppOpenDecisionWire | undefined =
    req.kind === "app.open" && req.reqId && req.nonce && req.machine && req.subject && req.appId
      ? { reqId: req.reqId, nonce: req.nonce, machine: req.machine, subject: req.subject, pkg: req.appId }
      : undefined;
  return {
    minutesGranted:
      req.kind === "time.extend"
        ? (minutesGranted ?? req.minutesRequested ?? 0)
        : req.kind === "app.open"
          ? (minutesGranted ?? 0)
          : undefined,
    // Compute exp in the LOCKED dimension's tz (see pickExtendTz) — never
    // the guardian phone's tz, which can overshoot the device's cap.
    tz: req.limitHit
      ? pickExtendTz(req.limitHit, scheduleTz, budgetTz, bucketsTz)
      : (scheduleTz ?? budgetTz ?? bucketsTz),
    wire,
    appOpen,
  };
}
