// The per-wrap intake shared by the relay poll and the home-network drain.
//
// A wrap addressed to the guardian arrives by one of two routes — read from a
// relay, or handed up by the Android carrier from a ward's direct link — and
// MUST be treated identically: same unwrap, same seal and machine-key checks
// (inside unwrapStatus / unwrapRequest), same wrap-id and reqId de-duplication,
// same freshness window. Local delivery can only change how the wrap got here,
// never what is checked about it. So both routes call this one function.

import type { NostrEvent } from "nostr-tools";
import {
  unwrapDeviceAudit,
  type DeviceAudit,
  type DeviceEmergencyCall,
  type DeviceEmergencyControl,
  type DeviceOverride,
} from "../wire/audit";
import { unwrapRequest, type DeviceRequest } from "../wire/request";
import { unwrapStatus, type DeviceStatus } from "../wire/status";

export interface WrapIngestContext {
  guardianSk: Uint8Array;
  /**
   * Wrap ids already dealt with this session — the CPU saver: a hit skips
   * decryption. The value is the audit key the wrap produced, when it produced
   * one: a second copy of the same wrap (the carrier's spooled one) then waits
   * for that line to be persisted before it is consumed, instead of being
   * consumed on the strength of a state that has not been written yet.
   */
  handledWraps: Map<string, string | undefined>;
  /**
   * Audits read and understood but not yet attributable (no paired device, and
   * no recently disconnected one, claims the machine), by wrap id. Held in
   * memory so the carrier's spool is not decrypted again every two seconds.
   * `sig` is `landingSig()` when the last attempt was made.
   */
  unlandedAudits: Map<string, { audit: DeviceAudit; sig: string }>;
  /**
   * A string that changes exactly when who an audit can be attributed to
   * changes (the children's devices, or the recently disconnected ones). A
   * cached unlanded audit is tried again only when it differs.
   */
  landingSig: () => string;
  /** reqIds already landed this session. */
  seenReqIds: Set<string>;
  /** Is this reqId already in the persisted queue (any status)? */
  hasRequest: (reqId: string) => boolean;
  /** Take in a ward heartbeat. */
  ingestStatus: (status: DeviceStatus) => void;
  /** Take in a ward ask; false = nobody to attribute it to yet (retry later). */
  ingestRequest: (request: DeviceRequest) => boolean;
  /**
   * Emergency unlocks (by `DeviceOverride.key`) landed this session. Needed
   * alongside `hasAudit` because several wraps can arrive in one tick, before
   * the state that holds the first has been updated.
   */
  seenAudits: Set<string>;
  /** Is this override already in the persisted Activity history? */
  hasAudit: (key: string) => boolean;
  /** Take in an emergency unlock; false = nobody to attribute it to yet (retry later). */
  ingestAudit: (audit: DeviceOverride) => boolean;
  /**
   * Take in a use of the emergency-call button; false = nobody to attribute it
   * to yet (retry later). Its keys (`DeviceEmergencyCall.key`) share `seenAudits`
   * and `hasAudit` with the unlocks: they are op-prefixed, so they never clash.
   */
  ingestEmergencyCall: (call: DeviceEmergencyCall) => boolean;
  /** Take in an emergency control this build does not know by name; false = retry later. */
  ingestEmergencyControl: (control: DeviceEmergencyControl) => boolean;
}

export type WrapIngestResult =
  /** Already handled earlier this session; nothing was decrypted. `auditKey`
   *  is the Activity line it produced, when it produced one. */
  | { kind: "handled"; auditKey?: string }
  | { kind: "status" }
  /** Not ours, or not readable. Neither will change with time. */
  | { kind: "unreadable" }
  /** An ask we already hold (this session or persisted). */
  | { kind: "duplicate"; reqId: string }
  /** A new ask, recorded now (it is persisted with the next state write). */
  | { kind: "request"; reqId: string }
  /** An emergency unlock or emergency-call press, new or already held: it is in the Activity history. */
  | { kind: "audit"; key: string }
  /** A ward's screen-lock reset audit: read, and nothing to record (STATUS carries the outcome). */
  | { kind: "screenlock" }
  /** An ask or emergency unlock from a phone this guardian has not recorded
   *  yet: left unhandled so a later pass can land it. `reqId` is the ask's. */
  | { kind: "unlanded"; reqId?: string };

/**
 * `verifiedAt`: when the carrier vetted this wrap, for an entry it held from a
 * relay. The device audit's two-day freshness window is then judged from that
 * moment, not from now, because the carrier already enforced it on arrival and
 * a guardian may open the console days later. Never later than now.
 */
export function ingestWrap(
  w: NostrEvent,
  ctx: WrapIngestContext,
  verifiedAt?: number,
): WrapIngestResult {
  const nowSecs = Math.floor(Date.now() / 1000);
  const auditNow =
    verifiedAt !== undefined && Number.isInteger(verifiedAt) && verifiedAt > 0
      ? Math.min(verifiedAt, nowSecs)
      : nowSecs;
  // The one line that saves the battery: everything below this decrypts.
  if (ctx.handledWraps.has(w.id)) return { kind: "handled", auditKey: ctx.handledWraps.get(w.id) };
  // An audit already read but not attributable: no decryption, and no new
  // attempt until who can be attributed has changed.
  const held = ctx.unlandedAudits.get(w.id);
  if (held) {
    const sig = ctx.landingSig();
    if (held.sig === sig) return { kind: "unlanded" };
    const result = landAudit(w, held.audit, ctx);
    if (result.kind === "unlanded") held.sig = sig;
    else ctx.unlandedAudits.delete(w.id);
    return result;
  }
  const s = unwrapStatus(w, ctx.guardianSk);
  if (s) {
    ctx.ingestStatus(s);
    ctx.handledWraps.set(w.id, undefined);
    return { kind: "status" };
  }
  const req = unwrapRequest(w, ctx.guardianSk);
  if (!req) {
    // Third and last, so the common status and request wraps never pay for a
    // third decrypt: the ward's emergency unlock (a device audit).
    const audit = unwrapDeviceAudit(w, ctx.guardianSk, auditNow);
    if (audit) {
      const result = landAudit(w, audit, ctx);
      if (result.kind === "unlanded") holdUnlanded(w.id, audit, ctx);
      return result;
    }
    // Not ours, or not readable. Neither will change with time.
    ctx.handledWraps.set(w.id, undefined);
    return { kind: "unreadable" };
  }
  if (ctx.seenReqIds.has(req.reqId) || ctx.hasRequest(req.reqId)) {
    ctx.handledWraps.set(w.id, undefined);
    return { kind: "duplicate", reqId: req.reqId };
  }
  // Mark seen only once it LANDED. This used to consume the id first, so
  // an ask that arrived while the ward's phone was missing from the
  // guardian's records was dropped by ingest and then skipped forever —
  // even though the relay keeps it for 24h and every poll re-reads that
  // window. A ward asking for time and getting permanent silence is the
  // worst version of this bug: she waits, and nobody was ever told.
  // Failing to consume it means the next poll heals it by itself the
  // moment her phone is set up — which is why an unlanded ask must also
  // hold the cursor open, or the narrowed window would take away the
  // second chance the wide one exists to give.
  if (ctx.ingestRequest(req)) {
    ctx.seenReqIds.add(req.reqId);
    ctx.handledWraps.set(w.id, undefined);
    return { kind: "request", reqId: req.reqId };
  }
  return { kind: "unlanded", reqId: req.reqId };
}

/** The most unlanded audits kept in memory; the oldest go first (they are read again from the carrier or relay if still needed). */
export const MAX_UNLANDED_AUDITS = 200;

export function holdUnlanded(id: string, audit: DeviceAudit, ctx: WrapIngestContext): void {
  ctx.unlandedAudits.set(id, { audit, sig: ctx.landingSig() });
  if (ctx.unlandedAudits.size > MAX_UNLANDED_AUDITS) {
    const oldest = ctx.unlandedAudits.keys().next().value;
    if (oldest !== undefined) ctx.unlandedAudits.delete(oldest);
  }
}

/** Land one authenticated, understood device audit. */
function landAudit(w: NostrEvent, audit: DeviceAudit, ctx: WrapIngestContext): WrapIngestResult {
  switch (audit.op) {
    case "unlock.breakglass":
      return ingestOverride(w, audit.override, ctx);
    case "call.emergency":
      return ingestEmergencyCall(w, audit.call, ctx);
    case "override.other":
      return ingestEmergencyControl(w, audit.control, ctx);
    case "screenlock.reset":
      // The ward's own record of a reset. The Screen lock card reads the
      // outcome from STATUS, so there is nothing to land here; it is
      // authenticated and understood, so it is done with.
      ctx.handledWraps.set(w.id, undefined);
      return { kind: "screenlock" };
  }
}

/**
 * An emergency unlock lands like an ask does: only once it is recorded, so one
 * from a phone this guardian has not set up yet stays unhandled and holds the
 * cursor open until it can be attributed. Silence about an emergency is the
 * worst outcome here.
 */
function ingestOverride(
  w: NostrEvent,
  audit: DeviceOverride,
  ctx: WrapIngestContext,
): WrapIngestResult {
  if (ctx.seenAudits.has(audit.key) || ctx.hasAudit(audit.key)) {
    ctx.handledWraps.set(w.id, audit.key);
    return { kind: "audit", key: audit.key };
  }
  if (!ctx.ingestAudit(audit)) return { kind: "unlanded" };
  ctx.seenAudits.add(audit.key);
  ctx.handledWraps.set(w.id, audit.key);
  return { kind: "audit", key: audit.key };
}

/**
 * A use of the emergency-call button lands exactly like an emergency unlock:
 * only once it is recorded, so one from a phone this guardian has not set up
 * yet stays unhandled and holds the cursor open until it can be attributed.
 */
function ingestEmergencyCall(
  w: NostrEvent,
  call: DeviceEmergencyCall,
  ctx: WrapIngestContext,
): WrapIngestResult {
  if (ctx.seenAudits.has(call.key) || ctx.hasAudit(call.key)) {
    ctx.handledWraps.set(w.id, call.key);
    return { kind: "audit", key: call.key };
  }
  if (!ctx.ingestEmergencyCall(call)) return { kind: "unlanded" };
  ctx.seenAudits.add(call.key);
  ctx.handledWraps.set(w.id, call.key);
  return { kind: "audit", key: call.key };
}

/** An emergency control this build does not know: lands exactly like a call press. */
function ingestEmergencyControl(
  w: NostrEvent,
  control: DeviceEmergencyControl,
  ctx: WrapIngestContext,
): WrapIngestResult {
  if (ctx.seenAudits.has(control.key) || ctx.hasAudit(control.key)) {
    ctx.handledWraps.set(w.id, control.key);
    return { kind: "audit", key: control.key };
  }
  if (!ctx.ingestEmergencyControl(control)) return { kind: "unlanded" };
  ctx.seenAudits.add(control.key);
  ctx.handledWraps.set(w.id, control.key);
  return { kind: "audit", key: control.key };
}
