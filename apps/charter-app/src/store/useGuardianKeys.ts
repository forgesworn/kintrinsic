// The store's half of *Guardian roots and working keys*: one `GuardianKeys`
// for the provider's lifetime, its persisted state kept in the household
// (`CharterState.guardianKeys`), and the actions screens call (the API batch
// 3b builds its screens on). The rules themselves live in
// `keys/guardianKeys.ts`; this file only wires state, keys and transport in.

import { useCallback, useEffect, useMemo, useRef, type MutableRefObject } from "react";
import { getPublicKey, SimplePool, type NostrEvent } from "nostr-tools";
import type { Child, SignerState } from "../domain/types";
import { GuardianKeys, type SendReport } from "../keys/guardianKeys";
import { reviveGuardianKeys, type GuardianKeysState, type InstallRoot, type RootVia } from "../keys/state";
import type { ChildReadiness } from "../keys/versionGate";
import type { GuardianOps } from "../wire/giftwrap";
import type { Signer } from "../signer/Signer";
import type { SelectSignerDeps } from "../signer/selectSigner";
import { loadOrCreateGuardianKey, readGuardianKey } from "../signer/guardianKey";
import { localRelayAuth } from "../signer/relayAuth";
import { withLocalRoute, type LocalRouteHooks } from "../signer/routePublish";
import { connectBunker } from "../signer/signetSigner";
import { DEFAULT_RELAYS } from "../signer/config";
import { devicesPinningLocalKey, keysChildren } from "./guardianKeysWiring";

/**
 * What screens use for guardian roots and working keys. Every action that
 * sends something resolves with a `SendReport` (which wards a relay or the
 * home network took it for, which are held, which failed) or throws with a
 * sentence a guardian can read.
 */
export interface GuardianKeysApi {
  /** The persisted state: this install's root, a move in progress, statements sent, PINs sent, the merged roster. */
  state: GuardianKeysState;
  /** W, this install's working key (hex); null when the local key can't be read. */
  workingPubkey: string | null;
  /** The key a ward pins for this install (R with a Signet root, else the local key). Pairing links and the SAS name it. */
  rootPubkey(): string;
  /** The versioning gate for one child. */
  childReadiness(childId: string): ChildReadiness;
  /** The gate across the family; co-guardians and moves are offered only when `ready`. */
  familyReadiness(): ReturnType<GuardianKeys["familyReadiness"]>;
  /**
   * Ask the connected root (My Signet, or a Heartwood behind it) to approve
   * this install's working key, then deliver the approval to every ward. The
   * joining parent's side passes `expectRoot` (their own identity, R2).
   */
  approveInstall(opts?: { expectRoot?: string; label?: string }): Promise<InstallRoot>;
  /** Invite another parent (npub or hex): R signs a co-guardian statement to every ward. */
  inviteCoGuardian(r2: string, opts?: { label?: string; expiresAt?: number }): Promise<SendReport>;
  /** Confirm a pending co-guardian, removal or move at a ward, from its presence code. */
  confirmPresence(code: string): Promise<SendReport>;
  /** Remove this install's own root from every ward (refused if it is the last root anywhere). */
  leaveFamily(): Promise<SendReport>;
  /** Start removing another parent: 72 hours' notice plus presence by this install at each ward. */
  removeCoGuardian(r2: string, opts?: { expiresAt?: number }): Promise<SendReport>;
  /** Object to a pending removal of this install's root (R signs and seals it). */
  objectToRemoval(d: string, remover?: string): Promise<SendReport>;
  /** Cancel a pending statement this install's root signed. */
  cancelPending(d: string): Promise<SendReport>;
  /** Move this install's root to the identity behind `bunkerUri` (presence at each ward finishes it). */
  moveRoot(bunkerUri: string, via?: RootVia): Promise<SendReport>;
  /** The move in progress: where to, whether it has lapsed, and which devices already switched. */
  moveProgress(): ReturnType<GuardianKeys["moveProgress"]>;
  /**
   * Restart a move that lapsed after some devices switched: the same new
   * identity, reconnected from its stored link. The one way out of a
   * half-way move (docs/guardian-keys-open-questions.md, "A root move that
   * lapses half-way").
   */
  finishMove(): Promise<SendReport>;
  /** Withdraw a working key for good (a lost phone). */
  withdrawInstall(w: string): Promise<SendReport>;
  /** Set the guardian PIN on Android wards (one child's, or all). */
  setGuardianPin(pin: string, opts?: { childId?: string; machines?: readonly string[] }): Promise<Omit<SendReport, "event">>;
  /** Share the family roster with the other guardians' working keys. */
  sendRoster(): ReturnType<GuardianKeys["sendRoster"]>;
  /** Roster devices this install has neither paired nor heard from. */
  unconfirmedRosterDevices(): ReturnType<GuardianKeys["unconfirmedRosterDevices"]>;
  /**
   * Trust the inviting parent's root: their guardian code (npub), scanned or
   * pasted. Throws a readable sentence for anything else. Returns the root.
   */
  anchorRoot(code: string): string;
  /** The roots this install shares the family with: its own, and those it anchored or invited. */
  trustedRoots(): string[];
  /** Wards confirmed in person here that wait for an anchored parent's family list. */
  waitingPresenceWards(): ReturnType<GuardianKeys["waitingPresenceWards"]>;
  /** Other parents whose family list stopped because they changed identity (scan their new code). */
  movedAwayRoots(): string[];
}

export interface UseGuardianKeysDeps {
  stateRef: MutableRefObject<{ children: Child[]; signer: SignerState; guardianKeys?: GuardianKeysState }>;
  /** Persist a new keys state. */
  save: (keys: GuardianKeysState) => void;
  /** The provider's current signer (read lazily: it is built after this hook runs). */
  signer: () => Signer | null;
  localRoute: LocalRouteHooks;
  /** Rule 4: re-sign one child's clause set; throws when it couldn't be sent. */
  resign: (childId: string) => Promise<void>;
  /** Record `key` as the pinned key of every paired device with none yet (see guardianKeysWiring). */
  stampPinnedKeys: (key: string, opts?: { onlyUnheard?: boolean }) => void;
  /** A root move landed on every ward: switch the signer to the new bunker. */
  onRootMoved: (bunkerUri: string) => void;
  /**
   * Re-open the paired bunker when the signer isn't connected (a family
   * paired with My Signet before this version, after a reload).
   */
  reconnect: () => Promise<void>;
}

/** Does this install read ward traffic with its local key? A local signer, or any install with an approved working key. */
export function listensWithLocalKey(s: { signer: SignerState; guardianKeys?: GuardianKeysState }): boolean {
  return s.signer.kind === "local" || !!s.guardianKeys?.root;
}

export function useGuardianKeys(deps: UseGuardianKeysDeps, children: Child[], keysState: GuardianKeysState | undefined) {
  const depsRef = useRef(deps);
  depsRef.current = deps;
  const keysRef = useRef<GuardianKeysState>(reviveGuardianKeys(deps.stateRef.current.guardianKeys));
  const pool = useRef<SimplePool | null>(null);

  const gk = useRef<GuardianKeys | null>(null);
  if (gk.current === null) {
    const relayPublish = async (relays: string[], event: NostrEvent): Promise<void> => {
      pool.current ??= new SimplePool();
      const sk = loadOrCreateGuardianKey();
      const results = await Promise.allSettled(pool.current.publish(relays, event, { onauth: localRelayAuth(sk) }));
      if (results.length > 0 && !results.some((r) => r.status === "fulfilled" && !String(r.value).startsWith("connection failure:"))) {
        throw new Error("Couldn't reach any relay — check your connection and try again.");
      }
    };
    const routed = withLocalRoute(relayPublish, depsRef.current.localRoute);
    gk.current = new GuardianKeys({
      read: () => ({ keys: keysRef.current, children: keysChildren(depsRef.current.stateRef.current.children, DEFAULT_RELAYS) }),
      write: (update) => {
        keysRef.current = update(keysRef.current);
        depsRef.current.save(keysRef.current);
      },
      workingKey: loadOrCreateGuardianKey,
      root: () => {
        const s = depsRef.current.signer() as (Signer & { rootOps?: () => GuardianOps | null }) | null;
        return s?.status().connected ? (s.rootOps?.() ?? null) : null;
      },
      publish: async (relays, wrap) => {
        await routed(relays, wrap);
      },
      now: () => Math.floor(Date.now() / 1000),
      defaultRelays: DEFAULT_RELAYS,
      resignClauses: (childId) => depsRef.current.resign(childId),
    });
  }
  const keys = gk.current;

  /** Hooks the Signet signer needs (see SignetSignerConfig). */
  const signerKeys = useMemo<NonNullable<SelectSignerDeps["keys"]>>(
    () => ({
      knownRoot: () => keysRef.current.root?.pubkey,
      workingKey: loadOrCreateGuardianKey,
      pickOps: (machine) => (keys.keyFor(machine) === "working" ? keys.workingOps() : undefined),
    }),
    [keys],
  );

  // Deliver W's approval where it's owed, and finish a root move once every
  // ward has it. Runs as STATUS changes what the wards report; the work is
  // rate-limited inside, so a heartbeat that changes nothing costs nothing.
  const syncing = useRef(false);
  const rootKey = keysState?.root?.pubkey;
  useEffect(() => {
    if (!rootKey || syncing.current) return;
    syncing.current = true;
    void (async () => {
      try {
        const moved = keys.settleRootMove();
        if (moved) depsRef.current.onRootMoved(moved);
        await keys.syncApprovals();
      } catch {
        // Offline, or no relay took it: the next change retries.
      } finally {
        syncing.current = false;
      }
    })();
  }, [children, rootKey, keys]);

  // An approval for a different working key (a restored key backup) is no
  // use: wards take an approval only from the W it names.
  useEffect(() => {
    keys.dropStaleApproval();
  }, [keys, rootKey]);

  // Presence wards expire: one a roster never named stops being admitted.
  useEffect(() => {
    keys.prunePresenceWards();
  }, [keys, children]);

  // Share the roster when what it says changes and another working key exists.
  const lastRoster = useRef<string | null>(null);
  useEffect(() => {
    if (!rootKey) return;
    const sig = JSON.stringify([
      keysChildren(children, DEFAULT_RELAYS).map((c) => [c.rosterId, c.name, c.colour, c.devices.map((d) => [d.machine, d.name])]),
      keys.reportedRoots(),
      keys.trustedRoots(),
    ]);
    if (sig === lastRoster.current) return;
    lastRoster.current = sig;
    void keys.sendRoster().catch(() => {
      lastRoster.current = null;
    });
  }, [children, rootKey, keys]);

  const approveInstall = useCallback<GuardianKeysApi["approveInstall"]>(
    async (opts = {}) => {
      let s = depsRef.current.stateRef.current;
      if (s.signer.kind !== "signet" && s.signer.kind !== "heartwood") {
        throw new Error("Connect My Signet first.");
      }
      if (!depsRef.current.signer()?.status().connected) {
        await depsRef.current.reconnect();
        s = depsRef.current.stateRef.current;
      }
      // A family already on My Signet: its devices with no record that it has
      // never heard from were offered by that root (their STATUS goes to R).
      // A phone switching from its own key recorded its devices as pinning
      // that key before connecting (see pairSignet).
      const r = depsRef.current.signer() as (Signer & { rootOps?: () => GuardianOps | null }) | null;
      const rootKey = r?.rootOps?.()?.pubkey;
      if (rootKey) {
        depsRef.current.stampPinnedKeys(rootKey, { onlyUnheard: true });
        s = depsRef.current.stateRef.current;
      }
      const w = keys.workingPubkey();
      const pinned = devicesPinningLocalKey(s.children, w);
      if (pinned.length > 0) {
        throw new Error(
          `${pinned.join(", ")} already trust the key on this phone. Moving them to My Signet is a guardian identity move, not a new connection.`,
        );
      }
      const root = await keys.approveThisInstall({
        via: s.signer.kind === "heartwood" ? "heartwood" : "signet",
        label: opts.label ?? keysRef.current.installLabel ?? "Kintrinsic",
        ...(opts.expectRoot ? { expectRoot: opts.expectRoot } : {}),
      });
      await keys.syncApprovals({ force: true }).catch(() => []);
      return root;
    },
    [keys],
  );

  const moveRoot = useCallback<GuardianKeysApi["moveRoot"]>(
    async (bunkerUri, via = "signet") => {
      pool.current ??= new SimplePool();
      const ops = await connectBunker(bunkerUri, pool.current);
      return keys.moveRoot({ ops, bunkerUri, via });
    },
    [keys],
  );

  const finishMove = useCallback<GuardianKeysApi["finishMove"]>(async () => {
    const next = keysRef.current.nextRoot;
    if (!next) throw new Error("There's no identity move to finish.");
    return moveRoot(next.bunkerUri, next.via);
  }, [moveRoot]);

  const workingPubkey = (() => {
    const k = readGuardianKey();
    return k.kind === "ok" ? getPublicKey(k.key) : null;
  })();

  const api = useMemo<GuardianKeysApi>(
    () => ({
      state: keysState ?? keysRef.current,
      workingPubkey,
      rootPubkey: () => keys.rootPubkey(),
      childReadiness: (id) => keys.childReadiness(id),
      familyReadiness: () => keys.familyReadiness(),
      approveInstall,
      inviteCoGuardian: (r2, opts) => keys.inviteCoGuardian(r2, opts),
      confirmPresence: (code) => keys.confirmPresence(code),
      leaveFamily: () => keys.leaveFamily(),
      removeCoGuardian: (r2, opts) => keys.removeCoGuardian(r2, opts),
      objectToRemoval: (d, remover) => keys.objectToRemoval(d, remover),
      cancelPending: (d) => keys.cancelPending(d),
      moveRoot,
      moveProgress: () => keys.moveProgress(),
      finishMove,
      withdrawInstall: (w) => keys.withdrawInstall(w),
      setGuardianPin: (pin, opts) => keys.setGuardianPin(pin, opts),
      sendRoster: () => keys.sendRoster(),
      unconfirmedRosterDevices: () => keys.unconfirmedRosterDevices(),
      anchorRoot: (code) => keys.anchorRoot(code),
      trustedRoots: () => keys.trustedRoots(),
      waitingPresenceWards: () => keys.waitingPresenceWards(),
      movedAwayRoots: () => keys.movedAwayRoots(),
    }),
    [keys, keysState, workingPubkey, approveInstall, moveRoot, finishMove],
  );

  return { keys, keysRef, signerKeys, api };
}
