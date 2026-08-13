// Subscribe to device REQUESTs on the relay: gift-wrapped (1059) events
// p-tagged to the guardian, unwrapped to kind-31111 DeviceRequests — the
// "child asked for more time" intake. This is the piece between the relay and
// the store — the unwrap/parse it delegates to is in ./request.ts. Malformed /
// wrong-recipient / spoofed events are silently dropped.

import type { NostrEvent } from "nostr-tools";
import { unwrapRequest, type DeviceRequest } from "./request";

const GIFT_WRAP = 1059;

/**
 * The slice of a relay pool we need. `nostr-tools`' `SimplePool` satisfies this
 * structurally; tests inject a fake. Kept minimal so this module never depends
 * on a live pool. (NB: unlike the older StatusSubPool sketch, `subscribeMany`
 * takes ONE filter — the actual SimplePool signature.)
 */
export interface RequestSubPool {
  subscribeMany(
    relays: string[],
    filter: { kinds?: number[]; "#p"?: string[] },
    params: { onevent: (e: NostrEvent) => void },
  ): { close: () => void };
}

export interface SubscribeRequestsOpts {
  relays: string[];
  /** The pinned guardian pubkey (hex) — the recipient the wraps are p-tagged to. */
  guardianPubkey: string;
  /** The guardian secret key used to decrypt (the D1 local self-signing path). */
  guardianSk: Uint8Array;
  /** Called once per DISTINCT reqId with each valid, unwrapped `DeviceRequest`. */
  onRequest: (request: DeviceRequest) => void;
  /** Unix seconds for the wrap-freshness check; defaults to the wall clock. */
  now?: () => number;
}

/**
 * Start a subscription for device REQUESTs. Returns a closer — call `.close()`
 * to unsubscribe.
 *
 * Re-delivery: relays replay stored wraps on reconnect and several relays may
 * hold the same wrap, so this dedupes by `reqId` for the subscription's
 * lifetime — `onRequest` fires at most once per ask. (The store additionally
 * dedupes against its persisted queue, covering reloads.)
 *
 * NOTE: decrypts with the LOCAL guardian key (the D1 self-signing default). A
 * Signet-bunker guardian would decrypt via NIP-46 `nip44Decrypt` instead — a
 * later seam; the unwrap logic (`unwrapRequest`) is otherwise identical.
 */
export function subscribeRequests(
  pool: RequestSubPool,
  opts: SubscribeRequestsOpts,
): { close: () => void } {
  const seen = new Set<string>();
  return pool.subscribeMany(
    opts.relays,
    { kinds: [GIFT_WRAP], "#p": [opts.guardianPubkey] },
    {
      onevent: (ev: NostrEvent) => {
        const request = unwrapRequest(ev, opts.guardianSk, opts.now?.());
        if (!request || seen.has(request.reqId)) return;
        seen.add(request.reqId);
        opts.onRequest(request);
      },
    },
  );
}
