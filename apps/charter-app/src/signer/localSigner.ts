import {
  SimplePool,
  finalizeEvent,
  getPublicKey,
  nip44,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import type { GuardianOps } from "../wire/giftwrap";
import type { Policy, SignerState } from "../domain/types";
import { RealSigner, type ChildTarget } from "./realSigner";
import { loadOrCreateGuardianKey } from "./guardianKey";
import type { ConfirmGate } from "./mockSigner";

export interface LocalSignerConfig {
  /** Resolve a child id → subject + device pubkeys + relays. */
  resolveChild: (childId: string) => ChildTarget | undefined;
  /** The child's current policy list (app-scope aggregation — see RealSigner). */
  childPolicies?: (childId: string) => Policy[];
  now?: () => number;
  /** Override the relay pool (tests). */
  pool?: SimplePool;
  /** Override the guardian key (tests). Defaults to the persisted browser key. */
  loadKey?: () => Uint8Array;
  /** In-app confirm gate (raises the approval sheet while auto-sign is off). */
  confirmGate?: ConfirmGate;
  /** Seed persisted signer state (e.g. autoSign) so a reload rebuild honors it. */
  initial?: SignerState;
}

/**
 * Build a guardian signer backed by a LOCAL BIP-340 key held in the browser —
 * Kintrinsic self-signs, no external bunker. It implements the same GuardianOps
 * the gift-wrap needs directly with nostr-tools; the laptop pins this key's
 * pubkey. This is the sibling of `signetSigner.ts` (which fulfils the same seam
 * over NIP-46). Orchestration + crypto live in realSigner.ts / wire/giftwrap.ts.
 */
export function createLocalSigner(config: LocalSignerConfig): RealSigner {
  const pool = config.pool ?? new SimplePool();
  const loadKey = config.loadKey ?? loadOrCreateGuardianKey;

  const connectGuardian = async (): Promise<GuardianOps> => {
    const sk = loadKey();
    const pubkey = getPublicKey(sk);
    return {
      pubkey,
      signEvent: async (t: EventTemplate): Promise<NostrEvent> => finalizeEvent(t, sk),
      nip44Encrypt: async (recipient: string, plaintext: string): Promise<string> =>
        nip44.encrypt(plaintext, nip44.getConversationKey(sk, recipient)),
    };
  };

  return new RealSigner({
    connectGuardian,
    publish: async (relays: string[], event: NostrEvent) => {
      // Fan out, but ONE accepted relay is the bar for "sent": settling every
      // promise and ignoring them all meant airplane mode read as success —
      // "she has a minute, then done for today" with nothing on any relay.
      // An empty attempt list (no relays configured) is left to
      // requireDeliverable's clearer message.
      const results = await Promise.allSettled(pool.publish(relays, event));
      if (results.length > 0 && !results.some((r) => r.status === "fulfilled")) {
        throw new Error("Couldn't reach any relay — check your connection and try again.");
      }
    },
    resolveChild: config.resolveChild,
    childPolicies: config.childPolicies,
    now: config.now ?? (() => Math.floor(Date.now() / 1000)),
    confirmGate: config.confirmGate,
    initial: config.initial,
  });
}
