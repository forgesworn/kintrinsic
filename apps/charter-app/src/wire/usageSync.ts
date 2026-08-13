// USAGE_SYNC (kind 31115) — the guardian-side consolidated cross-device usage
// view, one per device, so each warden can enforce the POOLED budget
// (spec/contract.md §USAGE_SYNC + the union-rule extension). Kintrinsic is the
// aggregator: it already receives every device's STATUS (usedTodaySecs +
// activeMinutesToday); this builds each device its "elsewhere" view — the
// OTHER devices' scalar sum and the union of their minute journals. Numbers +
// a bitmap only, never PII.
//
// TRUST BOUNDARY — read this before extending pooled budgets (S10, review
// 2026-08-07).
//
// The numbers aggregated here are SELF-REPORTED. A device's machine signature
// on its STATUS authenticates WHO is reporting; nothing authenticates that the
// figure is true. Everything downstream — this payload, the wardens' pooled
// enforcement, the guardian's weekly picture — inherits that.
//
// What it costs, precisely. A ward who extracts one device's machine key can
// under-report that device's `usedTodaySecs`, and every SIBLING device then
// grants them more of the shared pool than they are owed. The precondition is
// full compromise of a device the ward already controls — which by itself
// defeats enforcement ON that device, so the delta this buys them is purely
// cross-device. That is the honest size of it: real, bounded, and not a reason
// to distrust the ordinary case.
//
// Why it is not "fixed" here. There is no fix at this layer. A device is the
// only witness to its own screen time; a guardian's phone cannot see a laptop's
// foreground window. Any real defence has to make dishonesty *visible* rather
// than impossible — monotonic per-(device, day) counters so a total can never
// go backwards, and a regression flag the guardian is shown ("this device's
// figures went down") — which is a reflection surface, not a lock, and fits
// Kintrinsic's posture better than pretending the number is proof.
//
// Also unchecked here: `ingestDeviceStatus` accepts any `subject` a known
// machine reports, without confirming it names a child of this family. That is
// display-level confusion (a device could label its usage as someone else's),
// not budget theft, and it waits on `Child.dependantPubkey` ever being
// populated — see the pooled-budget/content-routing coupling.

import { decodeMinutes, emptyMinutes, encodeMinutes, unionMinutes } from "../insights/minuteSet";

/** Contract `UsageSyncPayload` (camelCase, v1). */
export interface UsageSyncPayload {
  v: 1;
  /** The child (subject pubkey hex). */
  subject: string;
  /** Guardian clock, unix SECONDS — receivers enforce strictly-monotonic. */
  ts: number;
  /** The local day this view describes (YYYY-MM-DD). */
  dayKey: string;
  /** Σ usedTodaySecs of the child's OTHER devices (receiver excluded). */
  spentElsewhereTodaySecs: number;
  weekKey?: string;
  spentElsewhereWeekSecs?: number;
  /** Union of the OTHER devices' active minutes (MinuteSet b64url). */
  elsewhereMinutesToday?: string;
}

/** One device's contribution to a child's day, as Kintrinsic knows it. */
export interface DeviceDayUsage {
  machine: string;
  secs: number;
  minutesB64?: string;
}

/**
 * Build the USAGE_SYNC payload for ONE receiving device from the child's full
 * device list for `dayKey`. The receiver is excluded (no double-count). The
 * union bitmap is included only when EVERY other device with usage has a valid
 * journal — a partial union would make the warden's union path undercount the
 * journal-less device, so we omit it and let the warden fall back to scalars.
 * Weekly fields are not yet published (wardens treat absence as local-only
 * weekly) — they arrive when the aggregator keeps week-keyed history.
 */
export function buildUsageSyncForDevice(opts: {
  subject: string;
  receiverMachine: string;
  dayKey: string;
  ts: number;
  devices: DeviceDayUsage[];
}): UsageSyncPayload {
  const others = opts.devices.filter((d) => d.machine !== opts.receiverMachine);
  const spentElsewhereTodaySecs = others.reduce(
    (s, d) => s + (Number.isFinite(d.secs) && d.secs > 0 ? Math.round(d.secs) : 0),
    0,
  );

  const contributing = others.filter((d) => d.secs > 0);
  let elsewhereMinutesToday: string | undefined;
  if (contributing.length > 0) {
    const journals = contributing.map((d) => (d.minutesB64 ? decodeMinutes(d.minutesB64) : null));
    if (journals.every((j) => j !== null)) {
      const union = (journals as Uint8Array[]).reduce(
        (acc, j) => unionMinutes(acc, j),
        emptyMinutes(),
      );
      elsewhereMinutesToday = encodeMinutes(union);
    }
  }

  const payload: UsageSyncPayload = {
    v: 1,
    subject: opts.subject,
    ts: opts.ts,
    dayKey: opts.dayKey,
    spentElsewhereTodaySecs,
  };
  if (elsewhereMinutesToday) payload.elsewhereMinutesToday = elsewhereMinutesToday;
  return payload;
}
