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
import { RealSigner, type ChildTarget, type RealSignerDeps } from "./realSigner";
import { localRelayAuth, relayAuthSigner } from "./relayAuth";
import { withLocalRoute, type LocalRouteHooks } from "./routePublish";

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
  /** Home-network delivery bookkeeping (device names, delivery tracking). */
  localRoute?: LocalRouteHooks;
  /**
   * Guardian roots and working keys: R (hex) once My Signet has approved this
   * install's working key. With it, the root's operations connect to the
   * bunker only when R is actually needed, so the working key signs and
   * relays authenticate without My Signet being reachable.
   */
  knownRoot?: () => string | undefined;
  /** This install's working key (the local guardian key), for relay AUTH once R has approved it. */
  workingKey?: () => Uint8Array;
  /** Per-ward key choice and the working-key PAIR_OFFER (see RealSignerDeps). */
  pickOps?: RealSignerDeps["pickOps"];
  pairOffer?: RealSignerDeps["pairOffer"];
}

/**
 * Build a Signet-backed signer. `connect()` opens a NIP-46 session to the paired
 * bunker; `signClause()` then signs + gift-wraps + publishes via the live relay
 * pool. The guardian private key stays in Signet — only `sign_event` /
 * `nip44_encrypt` cross the wire.
 */
export function createSignetSigner(config: SignetSignerConfig): RealSigner {
  const pool = config.pool ?? new SimplePool();
  let authenticate: ReturnType<typeof relayAuthSigner> | undefined;

  // One NIP-46 session, opened on first use and reopened after a failure.
  let session: Promise<{ bunker: BunkerSigner; pubkey: string }> | null = null;
  const bunkerSession = () => {
    if (!session) {
      session = (async () => {
        const uri = await config.getBunkerUri();
        const pointer = await parseBunkerInput(uri);
        if (!pointer) throw new Error("invalid bunker:// URI");
        const bunker = BunkerSigner.fromBunker(generateSecretKey(), pointer, { pool });
        await bunker.connect();
        const pubkey = await bunker.getPublicKey();
        authenticate = relayAuthSigner((template) => bunker.signEvent(template));
        return { bunker, pubkey };
      })();
      session.catch(() => {
        session = null;
      });
    }
    return session;
  };

  const connectGuardian = async (): Promise<GuardianOps> => {
    const known = config.knownRoot?.();
    if (known) {
      // R has approved this install's working key: connect lazily, and only
      // ever to the identity that approved it.
      const bunker = async () => {
        const s = await bunkerSession();
        if (s.pubkey !== known) throw new Error("My Signet is signed in as a different identity than this family's.");
        return s.bunker;
      };
      return {
        pubkey: known,
        signEvent: async (t: EventTemplate): Promise<NostrEvent> => (await bunker()).signEvent(t),
        nip44Encrypt: async (recipient: string, plaintext: string): Promise<string> =>
          (await bunker()).nip44Encrypt(recipient, plaintext),
      };
    }
    const { bunker, pubkey } = await bunkerSession();
    return {
      pubkey,
      signEvent: (t: EventTemplate): Promise<NostrEvent> => bunker.signEvent(t),
      nip44Encrypt: (recipient: string, plaintext: string): Promise<string> =>
        bunker.nip44Encrypt(recipient, plaintext),
    };
  };

  const relayPublish = async (relays: string[], event: NostrEvent): Promise<void> => {
    // Fan out, but ONE accepted relay is the bar for "sent" — same honesty
    // rule as localSigner: all-relays-refused must surface, not read as ok.
    // Any key in a ward's signer set may answer AUTH; once R has approved the
    // working key, that key answers without a round trip to My Signet.
    const w = config.knownRoot?.() && config.workingKey ? config.workingKey() : undefined;
    const onauth = w ? localRelayAuth(w) : authenticate;
    const results = await Promise.allSettled(pool.publish(relays, event, { onauth }));
    if (results.length > 0 && !results.some((r) => r.status === "fulfilled" && !String(r.value).startsWith("connection failure:"))) {
      throw new Error("Couldn't reach any relay — check your connection and try again.");
    }
  };

  return new RealSigner({
    connectGuardian,
    publish: withLocalRoute(relayPublish, config.localRoute),
    deviceName: config.localRoute?.deviceName,
    resolveChild: config.resolveChild,
    childPolicies: config.childPolicies,
    now: config.now ?? (() => Math.floor(Date.now() / 1000)),
    pickOps: config.pickOps,
    pairOffer: config.pairOffer,
  });
}
