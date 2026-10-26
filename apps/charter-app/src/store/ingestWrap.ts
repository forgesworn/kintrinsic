// The per-wrap intake shared by the relay poll and the home-network drain.
//
// A wrap addressed to the guardian arrives by one of two routes — read from a
// relay, or handed up by the Android carrier from a ward's direct link — and
// MUST be treated identically: same unwrap, same seal and machine-key checks
// (inside unwrapStatus / unwrapRequest), same wrap-id and reqId de-duplication,
// same freshness window. Local delivery can only change how the wrap got here,
// never what is checked about it. So both routes call this one function.

import type { NostrEvent } from "nostr-tools";
import { unwrapRequest, type DeviceRequest } from "../wire/request";
import { unwrapStatus, type DeviceStatus } from "../wire/status";

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
}

export type WrapIngestResult =
  /** Already handled earlier this session; nothing was decrypted. */
  | { kind: "handled" }
  | { kind: "status" }
  /** Not ours, or not readable. Neither will change with time. */
  | { kind: "unreadable" }
  /** An ask we already hold (this session or persisted). */
  | { kind: "duplicate"; reqId: string }
  /** A new ask, recorded now (it is persisted with the next state write). */
  | { kind: "request"; reqId: string }
  /** An ask from a phone this guardian has not recorded yet: left unhandled so
   *  a later pass can land it. */
  | { kind: "unlanded"; reqId: string };

export function ingestWrap(w: NostrEvent, ctx: WrapIngestContext): WrapIngestResult {
  // The one line that saves the battery: everything below this decrypts.
  if (ctx.handledWraps.has(w.id)) return { kind: "handled" };
  const s = unwrapStatus(w, ctx.guardianSk);
  if (s) {
    ctx.ingestStatus(s);
    ctx.handledWraps.add(w.id);
    return { kind: "status" };
  }
  const req = unwrapRequest(w, ctx.guardianSk);
  if (!req) {
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
