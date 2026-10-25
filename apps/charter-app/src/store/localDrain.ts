// Draining the carrier's home-network inbox into the console.
//
// The carrier holds wraps wards sent over their direct link until the console
// says it has them (`localConsume`). The rules (design: "Deduplication and
// ordering"):
//   - every wrap goes through the SAME intake as the relay poll (ingestWrap),
//     so the same wrap arriving by both routes collapses to one inbox entry;
//   - an id is only consumed once whatever it produced has been PERSISTED —
//     consuming first and crashing would lose an ask the carrier had safely
//     held;
//   - wraps that were rejected, unreadable or duplicates are consumed too, so
//     they do not come back every two seconds forever;
//   - an ask that could not land yet (unknown phone) is NOT consumed: the
//     carrier keeps it and a later drain retries, as the relay window does.

import type { NostrEvent } from "nostr-tools";
import type { LocalInboxEntry } from "../carrier/localLink";
import type { WrapIngestResult } from "./ingestWrap";

interface Pending {
  /** Present when the result is a new ask: don't consume until it is persisted. */
  reqId?: string;
}

/** Wrap ids waiting for the state that holds their result to reach storage. */
export class ConsumeQueue {
  private pending = new Map<string, Pending>();

  /** Queue an id. A pending ask requirement is never weakened by a later duplicate. */
  add(id: string, reqId?: string): void {
    const cur = this.pending.get(id);
    this.pending.set(id, { reqId: reqId ?? cur?.reqId });
  }

  get size(): number {
    return this.pending.size;
  }

  /**
   * Consume every queued id whose result is now persisted.
   *
   * `persistedOk` is whether the most recent write of the state to storage
   * succeeded; `hasRequest` asks that same state whether an ask is in it.
   */
  flush(opts: {
    persistedOk: boolean;
    hasRequest: (reqId: string) => boolean;
    consume: (ids: string[]) => void;
  }): void {
    if (!opts.persistedOk || this.pending.size === 0) return;
    const ready: string[] = [];
    for (const [id, p] of this.pending) {
      if (p.reqId === undefined || opts.hasRequest(p.reqId)) ready.push(id);
    }
    if (ready.length === 0) return;
    for (const id of ready) this.pending.delete(id);
    opts.consume(ready);
  }
}

function looksLikeWrap(w: unknown): w is NostrEvent {
  const e = w as NostrEvent | null;
  return (
    typeof e === "object" &&
    e !== null &&
    typeof e.id === "string" &&
    e.kind === 1059 &&
    typeof e.content === "string" &&
    Array.isArray(e.tags)
  );
}

/**
 * One pass over the inbox: feed each wrap through `ingest`, queue what can be
 * consumed. Returns how many entries were looked at.
 */
export function drainLocalInbox(deps: {
  inbox: () => LocalInboxEntry[] | null;
  ingest: (wrap: NostrEvent) => WrapIngestResult;
  queue: ConsumeQueue;
}): number {
  const entries = deps.inbox();
  if (!entries) return 0;
  for (const entry of entries) {
    if (!entry.id) continue;
    if (!looksLikeWrap(entry.wrap)) {
      // Malformed: nothing to ingest, and it would loop forever if left.
      deps.queue.add(entry.id);
      continue;
    }
    let result: WrapIngestResult;
    try {
      result = deps.ingest(entry.wrap);
    } catch {
      // Intake never throws on bad input; if something else did, retry later.
      continue;
    }
    if (result.kind === "unlanded") continue;
    // A duplicate waits for its ask too: the copy that landed it may have been
    // recorded this tick and not yet written to storage.
    deps.queue.add(
      entry.id,
      result.kind === "request" || result.kind === "duplicate" ? result.reqId : undefined,
    );
  }
  return entries.length;
}
