// What became of the wraps this console handed to the carrier for direct
// delivery on the home network — remembered, polled, and put into words.
//
// Design: docs/local-network-design.md, "Delivery state shown to the guardian".
// The carrier reports, per wrap and per ward device: queued, sent, received,
// then the ward's signed ACK result (applied, unchanged, superseded, expired,
// refused, failed, ignored). This module owns the console's small, bounded,
// persisted record of that, and the plain-language lines built from it.
//
// Honesty rules kept here, not in the screens:
//   - "Applied" appears only when the ward's own ACK says applied/unchanged.
//   - An app-open approval is "applied" only when the GRANT and that same
//     device's apps CLAUSE from the same approval both are.
//   - An install approval says "Approved on", never "installed": the ACK means
//     the approval was recorded and the install queued, not that Android
//     finished installing.

import { ACK_RESULTS, type LocalDeliveryState } from "../carrier/localLink";

export interface LocalWrapRecord {
  /** The wrap's event id. */
  wrap: string;
  /** The ward device's machine key (the wrap's recipient). */
  machine: string;
  deviceId?: string;
  childId?: string;
  kind: "grant" | "clause";
  /** The ask this wrap answers: set on a grant, and on the apps clause sent
   *  with an app-open approval. */
  reqId?: string;
  /** When the carrier accepted it (epoch ms). */
  at: number;
  state: LocalDeliveryState;
}

export const MAX_DELIVERY_RECORDS = 200;
export const MAX_DELIVERY_AGE_MS = 7 * 24 * 3600 * 1000;
/** The carrier keeps an undelivered wrap for 48 h; after that, stop asking. */
export const POLL_GIVE_UP_MS = 48 * 3600 * 1000;

const ALL_STATES: readonly string[] = ["queued", "sent", "received", "rejected", ...ACK_RESULTS];

/** True once nothing more will happen to the wrap: acked, or refused by the carrier. */
export function isFinalState(state: LocalDeliveryState): boolean {
  return state !== "queued" && state !== "sent" && state !== "received";
}

export function isApplied(state: LocalDeliveryState): boolean {
  return state === "applied" || state === "unchanged";
}

export function isProblem(state: LocalDeliveryState): state is "expired" | "refused" | "failed" {
  return state === "expired" || state === "refused" || state === "failed";
}

/** Read the persisted slice defensively — it came out of localStorage. */
export function recordsOf(list: unknown): LocalWrapRecord[] {
  if (!Array.isArray(list)) return [];
  return list.filter(
    (r): r is LocalWrapRecord =>
      typeof r === "object" &&
      r !== null &&
      typeof (r as LocalWrapRecord).wrap === "string" &&
      typeof (r as LocalWrapRecord).machine === "string" &&
      ((r as LocalWrapRecord).kind === "grant" || (r as LocalWrapRecord).kind === "clause") &&
      typeof (r as LocalWrapRecord).at === "number" &&
      ALL_STATES.includes((r as LocalWrapRecord).state),
  );
}

/** Keep the newest 200 within 7 days. */
export function pruneRecords(list: LocalWrapRecord[], now: number): LocalWrapRecord[] {
  const fresh = list.filter((r) => now - r.at <= MAX_DELIVERY_AGE_MS);
  const bounded = fresh.length > MAX_DELIVERY_RECORDS ? fresh.slice(fresh.length - MAX_DELIVERY_RECORDS) : fresh;
  return bounded.length === list.length ? list : bounded;
}

/** Add (or refresh) the record for a wrap, pruning as it goes. */
export function addRecord(list: LocalWrapRecord[], rec: LocalWrapRecord, now: number): LocalWrapRecord[] {
  const without = list.filter((r) => r.wrap !== rec.wrap);
  return pruneRecords([...without, rec], now);
}

/**
 * Fold the carrier's latest per-wrap states in. Returns the SAME array when
 * nothing changed, so a quiet poll causes no re-render. A final state is never
 * overwritten (a late "sent" must not undo an ACK).
 */
export function applyStates(
  list: LocalWrapRecord[],
  states: Record<string, { state: LocalDeliveryState }>,
): LocalWrapRecord[] {
  let changed = false;
  const next = list.map((r) => {
    const s = states[r.wrap]?.state;
    if (!s || s === r.state || isFinalState(r.state)) return r;
    changed = true;
    return { ...r, state: s };
  });
  return changed ? next : list;
}

/** Stamp the ask a set of already-recorded wraps belongs to. */
export function linkRecords(list: LocalWrapRecord[], wraps: string[], reqId: string): LocalWrapRecord[] {
  let changed = false;
  const next = list.map((r) => {
    if (!wraps.includes(r.wrap) || r.reqId === reqId) return r;
    changed = true;
    return { ...r, reqId };
  });
  return changed ? next : list;
}

/** Wrap ids worth asking the carrier about right now. */
export function idsToPoll(list: LocalWrapRecord[], now: number): string[] {
  return list.filter((r) => !isFinalState(r.state) && now - r.at <= POLL_GIVE_UP_MS).map((r) => r.wrap);
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

export interface DeliveryLine {
  tone: "ok" | "warn" | "info";
  text: string;
}

function problemText(device: string, result: "expired" | "refused" | "failed"): string {
  switch (result) {
    case "expired":
      return `${device} didn't act on this: the ask had already expired there.`;
    case "refused":
      return `${device} didn't accept this answer.`;
    case "failed":
      return `${device} couldn't carry this out. It may need another try.`;
  }
}

function waitingText(device: string, states: LocalDeliveryState[]): string {
  return states.includes("queued")
    ? `Waiting for ${device} to join the home network.`
    : `Sent to ${device} over the home network. Waiting for it to confirm.`;
}

/**
 * The line to show under an answered ask, or null when there is nothing honest
 * to add (the answer went by relays only, or the ward said something we have
 * no words for).
 */
export function answerDeliveryLine(
  req: { reqId?: string; kind: string; status: string },
  records: LocalWrapRecord[],
  nameOf: (rec: LocalWrapRecord) => string,
): DeliveryLine | null {
  if (!req.reqId) return null;
  const grant = records.find((r) => r.kind === "grant" && r.reqId === req.reqId);
  if (!grant) return null;
  const device = nameOf(grant);
  // The parts that must all land: the grant, and — for app.open — this same
  // device's apps clause(s) sent with the same approval.
  const parts =
    req.kind === "app.open" && req.status === "approved"
      ? [grant, ...records.filter((r) => r.kind === "clause" && r.reqId === req.reqId && r.machine === grant.machine)]
      : [grant];

  const problem = parts.find((r) => isProblem(r.state));
  if (problem && isProblem(problem.state)) {
    return { tone: "warn", text: problemText(device, problem.state) };
  }
  // app.open approved but no clause was tracked for this device: the hold went
  // by relays (or not at all) and nothing here can vouch for it.
  if (req.kind === "app.open" && req.status === "approved" && parts.length < 2) return null;

  if (parts.every((r) => isApplied(r.state))) {
    if (req.status === "denied") return { tone: "ok", text: `${device} has your answer.` };
    return {
      tone: "ok",
      text: req.kind === "install.app" ? `Approved on ${device}` : `Applied on ${device}`,
    };
  }
  if (parts.some((r) => r.state === "superseded" || r.state === "ignored" || r.state === "rejected")) {
    return null;
  }
  return { tone: "info", text: waitingText(device, parts.map((r) => r.state)) };
}

/** How long an answered ask keeps its delivery line on the Approvals screen. */
export const ANSWER_LINE_WINDOW_MS = 24 * 3600 * 1000;

/**
 * Recently answered asks that went by the home network, each with its line —
 * newest first, at most five. Asks answered by relay only have no record and
 * so no line.
 */
export function answeredLines<R extends { id: string; reqId?: string; kind: string; status: string }>(
  requests: R[],
  records: LocalWrapRecord[],
  nameOf: (rec: LocalWrapRecord) => string,
  now: number,
): { req: R; line: DeliveryLine }[] {
  const out: { req: R; line: DeliveryLine; at: number }[] = [];
  for (const req of requests) {
    if ((req.status !== "approved" && req.status !== "denied") || !req.reqId) continue;
    const grant = records.find((r) => r.kind === "grant" && r.reqId === req.reqId);
    if (!grant || now - grant.at > ANSWER_LINE_WINDOW_MS) continue;
    const line = answerDeliveryLine(req, records, nameOf);
    if (line) out.push({ req, line, at: grant.at });
  }
  return out
    .sort((a, b) => b.at - a.at)
    .slice(0, 5)
    .map(({ req, line }) => ({ req, line }));
}

/**
 * Devices (by machine key) on which every tracked clause wrap of a rule change
 * has been applied — for "— applied on {device}" on the activity line.
 */
export function ruleAppliedMachines(wraps: string[] | undefined, records: LocalWrapRecord[]): string[] {
  if (!wraps || wraps.length === 0) return [];
  const mine = records.filter((r) => r.kind === "clause" && wraps.includes(r.wrap));
  const machines = [...new Set(mine.map((r) => r.machine))];
  return machines.filter((m) => {
    const ofMachine = mine.filter((r) => r.machine === m);
    return ofMachine.every((r) => isApplied(r.state));
  });
}

/** " — applied on A" / " — applied on A and B" (empty when none). */
export function appliedSuffix(deviceNames: string[]): string {
  const names = [...new Set(deviceNames)];
  if (names.length === 0) return "";
  if (names.length === 1) return ` — applied on ${names[0]}`;
  return ` — applied on ${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}
