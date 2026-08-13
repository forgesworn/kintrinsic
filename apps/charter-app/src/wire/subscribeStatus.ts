// Subscribe to the device STATUS feed on the relay: gift-wrapped (1059) events
// p-tagged to the guardian, unwrapped to kind-31114 StatusPayloads. This is the
// piece between the relay and the store — the unwrap/parse it delegates to is in
// ./status.ts. Malformed / wrong-recipient / spoofed events are silently dropped.

import type { NostrEvent } from "nostr-tools";
import { unwrapStatus, type DeviceStatus } from "./status";

const GIFT_WRAP = 1059;

/**
 * The slice of a relay pool we need. `nostr-tools`' `SimplePool` satisfies this
 * structurally; tests inject a fake. Kept minimal so this module never depends
 * on a live pool.
 */
export interface StatusSubPool {
  subscribeMany(
    relays: string[],
    filter: { kinds?: number[]; "#p"?: string[] },
    params: { onevent: (e: NostrEvent) => void },
  ): { close: () => void };
}

export interface SubscribeStatusOpts {
  relays: string[];
  /** The pinned guardian pubkey (hex) — the recipient the wraps are p-tagged to. */
  guardianPubkey: string;
  /** The guardian secret key used to decrypt (the D1 local self-signing path). */
  guardianSk: Uint8Array;
  /** Called with each successfully unwrapped, valid `DeviceStatus`. */
  onStatus: (status: DeviceStatus) => void;
  /** Unix seconds for the wrap-freshness check; defaults to the wall clock. */
  now?: () => number;
}

/**
 * Start a subscription for the STATUS feed. Returns a closer — call `.close()`
 * to unsubscribe.
 *
 * NOTE: decrypts with the LOCAL guardian key (the D1 self-signing default). A
 * Signet-bunker guardian would decrypt via NIP-46 `nip44Decrypt` instead — a
 * later seam; the unwrap logic (`unwrapStatus`) is otherwise identical.
 */
export function subscribeStatus(
  pool: StatusSubPool,
  opts: SubscribeStatusOpts,
): { close: () => void } {
  return pool.subscribeMany(
    opts.relays,
    { kinds: [GIFT_WRAP], "#p": [opts.guardianPubkey] },
    {
      onevent: (ev: NostrEvent) => {
        const status = unwrapStatus(ev, opts.guardianSk, opts.now?.());
        if (status) opts.onStatus(status);
      },
    },
  );
}
