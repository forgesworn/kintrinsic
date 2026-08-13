// Production wiring: a RealSigner backed by the Signet bunker (NIP-46) + a relay
// pool. This is the thin transport adapter — the orchestration + crypto are in
// realSigner.ts / wire/giftwrap.ts (unit-tested). The live bunker/relay IO here
// is exercised against a running Signet, not in the headless suite.

import {
  generateSecretKey,
  SimplePool,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import { BunkerSigner, parseBunkerInput } from "nostr-tools/nip46";
import type { GuardianOps } from "../wire/giftwrap";
import type { Policy } from "../domain/types";
import { RealSigner, type ChildTarget } from "./realSigner";

export interface SignetSignerConfig {
  /** The paired guardian `bunker://…` URI (from the pairing flow). */
  getBunkerUri: () => Promise<string>;
  /** Resolve a child id → subject + device pubkeys + relays. */
  resolveChild: (childId: string) => ChildTarget | undefined;
  /** The child's current policy list (app-scope aggregation — see RealSigner). */
  childPolicies?: (childId: string) => Policy[];
  now?: () => number;
  /** Override the relay pool (tests). */
  pool?: SimplePool;
}

/**
 * Build a Signet-backed signer. `connect()` opens a NIP-46 session to the paired
 * bunker; `signClause()` then signs + gift-wraps + publishes via the live relay
 * pool. The guardian private key stays in Signet — only `sign_event` /
 * `nip44_encrypt` cross the wire.
 */
export function createSignetSigner(config: SignetSignerConfig): RealSigner {
  const pool = config.pool ?? new SimplePool();

  const connectGuardian = async (): Promise<GuardianOps> => {
    const uri = await config.getBunkerUri();
    const pointer = await parseBunkerInput(uri);
    if (!pointer) throw new Error("invalid bunker:// URI");
    const bunker = BunkerSigner.fromBunker(generateSecretKey(), pointer, { pool });
    await bunker.connect();
    const pubkey = await bunker.getPublicKey();
    return {
      pubkey,
      signEvent: (t: EventTemplate): Promise<NostrEvent> => bunker.signEvent(t),
      nip44Encrypt: (recipient: string, plaintext: string): Promise<string> =>
        bunker.nip44Encrypt(recipient, plaintext),
    };
  };

  return new RealSigner({
    connectGuardian,
    publish: async (relays: string[], event: NostrEvent) => {
      // Fan out, but ONE accepted relay is the bar for "sent" — same honesty
      // rule as localSigner: all-relays-refused must surface, not read as ok.
      const results = await Promise.allSettled(pool.publish(relays, event));
      if (results.length > 0 && !results.some((r) => r.status === "fulfilled")) {
        throw new Error("Couldn't reach any relay — check your connection and try again.");
      }
    },
    resolveChild: config.resolveChild,
    childPolicies: config.childPolicies,
    now: config.now ?? (() => Math.floor(Date.now() / 1000)),
  });
}
