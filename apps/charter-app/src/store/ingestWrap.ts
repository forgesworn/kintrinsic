// The per-wrap intake shared by the relay poll and the home-network drain.
//
// A wrap addressed to the guardian arrives by one of two routes — read from a
// relay, or handed up by the Android carrier from a ward's direct link — and
// MUST be treated identically: same unwrap, same seal and machine-key checks
// (inside unwrapStatus / unwrapRequest), same wrap-id and reqId de-duplication,
// same freshness window. Local delivery can only change how the wrap got here,
// never what is checked about it. So both routes call this one function.

import type { NostrEvent } from "nostr-tools";
import { unwrapAudit, type DeviceOverride } from "../wire/audit";
import { unwrapRequest, type DeviceRequest } from "../wire/request";
import { unwrapStatusOrFarewell, type DeviceStatus, type StatusFarewell } from "../wire/status";
import { MAX_WRAP_JITTER_SECS } from "../wire/request";
import { unwrapToWorkingKey } from "../keys/guardianKeys";
import { CHARTER_FAMILY_ROSTER, type SignedEvent } from "../keys/sdk";

export interface WrapIngestContext {
  guardianSk: Uint8Array;
  /** Wrap ids already dealt with this session — the CPU saver: a hit skips decryption. */
  handledWraps: Set<string>;
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
  /** A departed key's last STATUS (guardian keys). Never usage data. */
  ingestFarewell?: (farewell: StatusFarewell) => void;
  /** A family roster (31119) from another guardian install's working key. */
  ingestRoster?: (inbound: { sealAuthor: string; inner: SignedEvent }) => void;
}

export type WrapIngestResult =
  /** Already handled earlier this session; nothing was decrypted. */
  | { kind: "handled" }
  | { kind: "status" }
  /** A departed key's last STATUS, or a family roster. */
  | { kind: "farewell" }
  | { kind: "roster" }
  /** Not ours, or not readable. Neither will change with time. */
  | { kind: "unreadable" }
  /** An ask we already hold (this session or persisted). */
  | { kind: "duplicate"; reqId: string }
  /** A new ask, recorded now (it is persisted with the next state write). */
  | { kind: "request"; reqId: string }
  /** An emergency unlock, new or already held: it is in the Activity history. */
  | { kind: "audit"; key: string }
  /** An ask or emergency unlock from a phone this guardian has not recorded
   *  yet: left unhandled so a later pass can land it. `reqId` is the ask's. */
  | { kind: "unlanded"; reqId?: string };

export function ingestWrap(w: NostrEvent, ctx: WrapIngestContext): WrapIngestResult {
  // The one line that saves the battery: everything below this decrypts.
  if (ctx.handledWraps.has(w.id)) return { kind: "handled" };
  const opened = unwrapStatusOrFarewell(w, ctx.guardianSk);
  if (opened && "status" in opened) {
    ctx.ingestStatus(opened.status);
    ctx.handledWraps.add(w.id);
    return { kind: "status" };
  }
  if (opened && ctx.ingestFarewell) {
    // A departed key's last STATUS: never usage, only where this phone stands.
    ctx.ingestFarewell(opened.farewell);
    ctx.handledWraps.add(w.id);
    return { kind: "farewell" };
  }
  const req = unwrapRequest(w, ctx.guardianSk);
  // A roster from another guardian install, inside the same freshness window
  // the other intakes keep.
  const fresh = Math.abs(Math.floor(Date.now() / 1000) - w.created_at) <= MAX_WRAP_JITTER_SECS;
  if (!req && ctx.ingestRoster && fresh) {
    const roster = unwrapToWorkingKey(w, ctx.guardianSk);
    if (roster && roster.inner.kind === CHARTER_FAMILY_ROSTER) {
      ctx.ingestRoster(roster);
      ctx.handledWraps.add(w.id);
      return { kind: "roster" };
    }
  }
  if (!req) {
    // Third and last, so the common status and request wraps never pay for a
    // third decrypt: the ward's emergency unlock (a device audit).
    const audit = unwrapAudit(w, ctx.guardianSk);
    if (audit) return ingestOverride(w, audit, ctx);
    // Not ours, or not readable. Neither will change with time.
    ctx.handledWraps.add(w.id);
    return { kind: "unreadable" };
  }
  if (ctx.seenReqIds.has(req.reqId) || ctx.hasRequest(req.reqId)) {
    ctx.handledWraps.add(w.id);
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
    ctx.handledWraps.add(w.id);
    return { kind: "request", reqId: req.reqId };
  }
  return { kind: "unlanded", reqId: req.reqId };
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
    ctx.handledWraps.add(w.id);
    return { kind: "audit", key: audit.key };
  }
  if (!ctx.ingestAudit(audit)) return { kind: "unlanded" };
  ctx.seenAudits.add(audit.key);
  ctx.handledWraps.add(w.id);
  return { kind: "audit", key: audit.key };
}
