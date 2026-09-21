import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type {
  ActivityEvent,
  ActivityOutcome,
  Budget,
  Child,
  ChildRequest,
  Device,
  PairingState,
  Policy,
  RequestKind,
  Schedule,
  SignerKind,
  SignerState,
  WebPolicy,
  AppsPolicy,
  BucketsPolicy,
  LearningPolicy,
  ListeningPolicy,
  Lifeline,
  Tethering,
  AlwaysAvailablePolicy,
} from "../domain/types";
import {
  SPLITTABLE_CONTROLS,
  pickOverrides,
  type PolicyOverride,
} from "../domain/effectivePolicy";
import type { FreeGroupRecord } from "../domain/namedTimes";
import { resolveChildTz } from "../domain/childTz";
import { composeDeviceHold } from "../domain/appHolds";
import { raceRefresh, REFRESH_OFFLINE_NOTICE } from "../domain/refreshDeadline";
import { appOpenRequestToCard, timeExtendRequestToCard } from "../domain/requestCards";
import { identityDisplayLabel } from "../domain/launchSignatures";
import { decisionTiming } from "./decisionTiming";
import { devicesToRelease } from "./releaseOnRemove";
import { runApproveAppOpenFlow } from "./approveAppOpenFlow";
import { getPublicKey, SimplePool } from "nostr-tools";
import { fetchAllReleaseManifests } from "../release/fetchReleases";
import { pickInstallUrl, type ReleaseManifest } from "../release/releaseEvent";
import { SignerCancelled, type ConfirmGate } from "../signer/mockSigner";
import type { DecisionContext, InstallDecision, Signer } from "../signer/Signer";
import { catalogLookup } from "../data/appCatalog";
import {
  guardianPubkeyHex,
  importGuardianKey,
  loadOrCreateGuardianKey,
} from "../signer/guardianKey";
import { resolveChildTarget } from "../signer/resolveChild";
import {
  clauseDeliveryPrecheck,
  undeliveredNote,
  type ClauseDelivery,
} from "./clauseDelivery";
import { unclaimedDevices, type UnclaimedDevice } from "./unclaimedDevices";
import { forgetPairToken, livePairTokens, rememberPairToken } from "./pairTokens";
import { readyResends, type PendingResend } from "./resendOnClaim";
import { DEFAULT_RELAYS } from "../signer/config";
import { standingFor, standingNote } from "../domain/standing";
import { selectSigner } from "../signer/selectSigner";
import { mintPairToken } from "../signer/guardianPairing";
import { parseDevicePairingCode } from "../wire/deviceCode";
import type { AppOpenWindow } from "../wire/grant";
import { unwrapRequest, type DeviceRequest } from "../wire/request";
import { unwrapStatus, type DeviceStatus } from "../wire/status";
import type { UpdateManifest } from "../wire/types";
import { fetchManifest, fetchDebManifest, APK_PATH, type DebManifest } from "./updateCheck";
import { makeEmpty, makeSeed } from "../data/seed";
import { decisionPath } from "./decisionGate";
import { nextCursor, windowSince } from "./relayWindow";
import { statusFor, type StatusForResult } from "./statusFor";
import {
  emptyUsageHistory,
  pruneHistory,
  recordUsage,
  type UsageHistory,
} from "../insights/usageHistory";
import { buildUsageSyncForDevice } from "../wire/usageSync";

export { isSetUp, statusFor } from "./statusFor";
export type { StatusForResult } from "./statusFor";
export { liveStatusFor, freshestStatusFor } from "./liveStatus";
export type { DeviceStatus } from "../wire/status";

// ---------------------------------------------------------------------------
// State + persistence
// ---------------------------------------------------------------------------

export interface CharterState {
  children: Child[];
  requests: ChildRequest[];
  activity: ActivityEvent[];
  signer: SignerState;
}

// v2: the production default is now an empty (clean) state, so bump the key —
// every prior visitor was seeded with the demo family under v1; reading a fresh
// key gives them the real onboarding on next load instead of stale sample data.
// Exported so tests can seed/read the exact key `persist`/`loadState` use,
// rather than duplicating the literal string.
export const STORAGE_KEY = "charter.state.v2";
const USAGE_KEY = "charter.usage.v1";

/** The activity-log phrase for the one-tap app.open window a guardian just
 *  picked (`approveAppOpen`). */
const APP_OPEN_WINDOW_PHRASE: Record<AppOpenWindow, string> = {
  "30m": "for 30 minutes",
  "1h": "for 1 hour",
  restOfDay: "until tonight",
};

/** Ceiling on the remembered-wrap set. Comfortably more than a busy day of
 *  heartbeats and asks, and bounded so a session left open for a week cannot
 *  grow it without limit. */
const HANDLED_WRAPS_MAX = 4000;

/** Drop the oldest ids once the set is over its ceiling. Sets iterate in
 *  insertion order, so the front of it is the oldest thing we know about — and
 *  anything dropped is at worst decrypted once more, never lost. */
function trimHandled(handled: Set<string>): void {
  if (handled.size <= HANDLED_WRAPS_MAX) return;
  const excess = handled.size - HANDLED_WRAPS_MAX;
  let dropped = 0;
  for (const id of handled) {
    if (dropped >= excess) break;
    handled.delete(id);
    dropped += 1;
  }
}

function loadState(): CharterState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return JSON.parse(raw) as CharterState;
  } catch {
    // Corrupt or unavailable storage — fall through to the default below.
  }
  // Production starts CLEAN (straight into "Add your first child"); the sample
  // family is a dev-only convenience so the UI is explorable while building.
  return import.meta.env.PROD ? makeEmpty() : makeSeed();
}

/** Raw persisted-state passthrough for the FULL backup (key + household). */
export function exportPersistedState(): string | null {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

/** Once a restored household is written, the live (pre-restore) in-memory
 *  state must NEVER persist over it — the persist-on-change effect fires on
 *  the signer update from restoreGuardianKey and would clobber the restore
 *  (found on-metal 2026-07-23: "still prompting for my first child's name").
 *  Frozen until the reload that follows every restore. */
let persistenceFrozen = false;

/** Install a restored household state. Throws on garbage BEFORE clobbering;
 *  freezes further persistence — the caller reloads the app so the store
 *  boots from the restored state. */
export function importPersistedState(json: string): void {
  const parsed: unknown = JSON.parse(json);
  if (typeof parsed !== "object" || parsed === null || !Array.isArray((parsed as { children?: unknown }).children)) {
    throw new Error("that backup's family data is not usable");
  }
  persistenceFrozen = true;
  localStorage.setItem(STORAGE_KEY, json);
}

function persist(state: CharterState): void {
  if (persistenceFrozen) return; // a restored household is on disk — reload owns the truth
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Storage full / private mode — non-fatal for the demo.
  }
}

// ---------------------------------------------------------------------------
// Reducer
// ---------------------------------------------------------------------------

type Action =
  | { type: "ADD_CHILD"; child: Child }
  | { type: "EDIT_CHILD"; childId: string; name: string; color?: string }
  | { type: "REMOVE_CHILD"; childId: string }
  | { type: "ADD_DEVICE"; childId: string; device: Device }
  | {
      type: "SET_DEVICE_PAIRING";
      childId: string;
      deviceId: string;
      pairing: PairingState;
      pairedAt?: number;
      devicePubkey?: string | null;
    }
  | { type: "REMOVE_DEVICE"; childId: string; deviceId: string }
  | { type: "SET_DEVICE_LAST_SEEN"; machine: string; lastSeenAt: number }
  | { type: "SET_POLICY"; childId: string; policy: Policy }
  | { type: "ADD_POLICY"; childId: string; policy: Policy }
  | { type: "REMOVE_POLICY"; childId: string; policyId: string }
  | { type: "ADD_REQUEST"; request: ChildRequest }
  | { type: "SET_REQUEST_STATUS"; id: string; status: ChildRequest["status"] }
  | { type: "SET_REQUEST_CHOSEN_MINUTES"; id: string; minutes: number }
  | { type: "ADD_ACTIVITY"; event: ActivityEvent }
  | { type: "SET_SIGNER"; signer: SignerState };

function mapDevice(
  child: Child,
  deviceId: string,
  fn: (d: Device) => Device,
): Child {
  return {
    ...child,
    devices: child.devices.map((d) => (d.id === deviceId ? fn(d) : d)),
  };
}

function upsertPolicy(child: Child, policy: Policy): Child {
  const exists = child.policies.some((p) => p.id === policy.id);
  const policies = exists
    ? child.policies.map((p) => (p.id === policy.id ? policy : p))
    : [...child.policies, policy];
  return { ...child, policies };
}

// Exported for `store.test.ts` — a plain, side-effect-free state transition
// function, so the M-1 "chosen amount survives a failed send + remount"
// regression can be exercised at the data layer without rendering React.
export function reducer(state: CharterState, action: Action): CharterState {
  switch (action.type) {
    case "ADD_CHILD":
      return { ...state, children: [...state.children, action.child] };
    case "EDIT_CHILD":
      return {
        ...state,
        children: state.children.map((c) =>
          c.id === action.childId
            ? { ...c, name: action.name, color: action.color ?? c.color }
            : c,
        ),
      };
    case "REMOVE_CHILD":
      return {
        ...state,
        children: state.children.filter((c) => c.id !== action.childId),
        requests: state.requests.filter((r) => r.childId !== action.childId),
      };
    case "ADD_DEVICE":
      return {
        ...state,
        children: state.children.map((c) =>
          c.id === action.childId
            ? { ...c, devices: [...c.devices, action.device] }
            : c,
        ),
      };
    case "SET_DEVICE_PAIRING":
      return {
        ...state,
        children: state.children.map((c) =>
          c.id === action.childId
            ? mapDevice(c, action.deviceId, (d) => ({
                ...d,
                pairing: action.pairing,
                pairedAt: action.pairedAt ?? d.pairedAt,
                devicePubkey:
                  action.devicePubkey !== undefined
                    ? action.devicePubkey
                    : d.devicePubkey,
              }))
            : c,
        ),
      };
    case "REMOVE_DEVICE":
      return {
        ...state,
        children: state.children.map((c) =>
          c.id === action.childId
            ? { ...c, devices: c.devices.filter((d) => d.id !== action.deviceId) }
            : c,
        ),
      };
    // Keyed by machine pubkey (the STATUS feed's identity), not deviceId — a
    // heartbeat arrives attributed only to the emitting device's key.
    case "SET_DEVICE_LAST_SEEN":
      // Monotonic: "last seen" only ever moves forward. Relay/poll delivery is
      // not guaranteed chronological (the loop re-queries a rolling window every
      // tick), so an older STATUS arriving after a newer one must not drag the
      // timestamp backwards — that would flicker a live device as "gone dark",
      // the exact audit-gap signal this field exists to provide.
      return {
        ...state,
        children: state.children.map((c) =>
          c.devices.some(
            (d) =>
              d.devicePubkey === action.machine &&
              action.lastSeenAt > (d.lastSeenAt ?? 0),
          )
            ? {
                ...c,
                devices: c.devices.map((d) =>
                  d.devicePubkey === action.machine &&
                  action.lastSeenAt > (d.lastSeenAt ?? 0)
                    ? { ...d, lastSeenAt: action.lastSeenAt }
                    : d,
                ),
              }
            : c,
        ),
      };
    case "SET_POLICY":
    case "ADD_POLICY":
      return {
        ...state,
        children: state.children.map((c) =>
          c.id === action.childId ? upsertPolicy(c, action.policy) : c,
        ),
      };
    case "REMOVE_POLICY":
      return {
        ...state,
        children: state.children.map((c) =>
          c.id === action.childId
            ? {
                ...c,
                policies: c.policies.filter((p) => p.id !== action.policyId),
              }
            : c,
        ),
      };
    case "ADD_REQUEST":
      return { ...state, requests: [action.request, ...state.requests] };
    case "SET_REQUEST_STATUS":
      return {
        ...state,
        requests: state.requests.map((r) =>
          r.id === action.id ? { ...r, status: action.status } : r,
        ),
      };
    case "SET_REQUEST_CHOSEN_MINUTES":
      return {
        ...state,
        requests: state.requests.map((r) =>
          r.id === action.id ? { ...r, chosenMinutes: action.minutes } : r,
        ),
      };
    case "ADD_ACTIVITY":
      return { ...state, activity: [action.event, ...state.activity] };
    case "SET_SIGNER":
      return { ...state, signer: action.signer };
    default:
      return state;
  }
}

// ---------------------------------------------------------------------------
// Context surface
// ---------------------------------------------------------------------------

export interface PendingSignature {
  action: "clause" | "decision";
  /** Present iff `action === "decision"` — which way the guardian's request
   *  decision went, so the sheet's CTA never says "Approve" for a no. */
  decision?: "approved" | "denied";
  signerLabel: string;
  /** The signer's identity signal (`"local"` etc.) — see signPrompt.ts's
   *  `isLocal`; prefer this over pattern-matching `signerLabel`. */
  signerKind: SignerKind;
  detail: string;
}

export interface CharterContextValue {
  state: CharterState;

  // Family
  addChild: (name: string, color?: string) => Child;
  editChild: (childId: string, name: string, color?: string) => void;
  removeChild: (childId: string) => void;

  // Devices (parent-governed pairing)
  addDevice: (
    childId: string,
    label: string,
    platform?: Device["platform"],
  ) => Device;
  beginPairing: (childId: string, deviceId: string) => void;
  /** Binds a REAL device code (hex/npub/nprofile) to a just-added device;
   *  undefined = code refused. Pass the `Device` returned by `addDevice` (not
   *  just its id) — it was created in the same tick and is not yet visible in
   *  committed state. */
  confirmPairing: (
    childId: string,
    device: Device,
    scannedCode: string,
  ) => Device | undefined;
  /** Re-pair using the stored real key; undefined = no real key to reuse. */
  reconnectDevice: (childId: string, deviceId: string) => Device | undefined;
  /**
   * Adopt a phone we have heard from but never recorded — the far side of a
   * split-brain pairing. Undefined if the key is malformed or already claimed.
   */
  claimDevice: (childId: string, machine: string, label: string) => Device | undefined;
  /** Phones heartbeating at this guardian that no child claims (see ./unclaimedDevices). */
  unclaimed: UnclaimedDevice[];
  /** Freshest live STATUS per machine pubkey (the devices' real reports). */
  deviceStatus: Record<string, DeviceStatus>;
  /** Per-device, per-day screen-time history (design memo B1 — the weekly picture). */
  usageHistory: UsageHistory;
  /** The site's published Kintrinsic artifact manifest; null until fetched (#44). */
  updateManifest: UpdateManifest | null;
  /** The published Kintrinsic-for-Linux artifact, for paired laptops. */
  debManifest: DebManifest | null;
  /** Re-read device reports AND the published manifests. Bound to focus and to
   *  pull-to-refresh; safe to call as often as you like (every intake is
   *  idempotent). */
  refreshNow: () => Promise<void>;
  /** A refresh is in flight — for the pull-to-refresh spinner. */
  refreshing: boolean;
  /** Set when the last refresh could not reach the relays (offline, or they're
   *  down — indistinguishable, so one honest line). Clears itself. */
  refreshNotice: string | null;
  /** Give a child minutes with no ask outstanding (and after a deny). Today
   *  only. `groupId` tops up that named-times group's own pool instead of
   *  the whole-device one. */
  giveTime: (childId: string, minutes: number, groupId?: string) => Promise<boolean>;
  /** Ask a ward to finish up now (or, with `lift`, allow them back on). */
  standDown: (childId: string, lift?: boolean) => Promise<boolean>;
  /** Open a short, signed window during which the ward drops its install lock. */
  openMaintenanceWindow: (childId: string, minutes: number) => Promise<boolean>;
  /** Publish the `update` clause to every governed device the child uses. */
  sendCharterUpdate: (childId: string) => Promise<boolean>;
  /** QR onboarding: the outstanding pairing wait, if a QR is on screen. */
  phonePairing: { childId: string; token: string; foundMachine: string | null } | null;
  /** Mint a one-time token and start watching for its STATUS echo. */
  beginPhonePairing: (childId: string) => string;
  cancelPhonePairing: () => void;
  /** Demo/seed only: add a device and mock-pair it (non-deliverable target). */
  enrollDevice: (childId: string, label: string) => Device;
  unpairDevice: (childId: string, deviceId: string) => void;
  /** Scan-to-pair: offer to be pinned by a freshly-scanned ward. */
  sendPairOffer: (machine: string, token: string) => Promise<void>;
  removeDevice: (childId: string, deviceId: string) => void;

  // Stage 1 intake — mock a device-originated ask (demo + seed parity)
  simulateDeviceRequest: (
    childId: string,
    deviceId: string,
    kind: RequestKind,
    opts?: {
      appLabel?: string;
      appId?: string;
      minutesRequested?: number;
      reason?: string;
      title?: string;
      limitHit?: "schedule" | "budget";
    },
  ) => ChildRequest;

  // Limits (rule changes — signed when a signer is connected)
  setSchedule: (childId: string, scopeId: string, schedule: Schedule) => Promise<void>;
  setBudget: (childId: string, scopeId: string, budget: Budget) => Promise<void>;
  /** Save schedule and/or budget and/or web policy for one scope in a SINGLE
   *  signed change (shared issuedAt), so saving several never reverts one. */
  savePolicy: (
    childId: string,
    scopeId: string,
    next: {
      schedule?: Schedule;
      budget?: Budget;
      web?: WebPolicy;
      apps?: AppsPolicy;
      learning?: LearningPolicy;
      tethering?: Tethering;
      lifeline?: Lifeline;
      buckets?: BucketsPolicy;
      listening?: ListeningPolicy;
      /** Apps open at any hour, standing or expiring (2026-08-03). */
      alwaysAvailable?: AlwaysAvailablePolicy;
      /** Per-device divergence (Half A). Absent = one charter for every device. */
      deviceOverrides?: Record<string, PolicyOverride>;
      /** Named-times Free-group membership — guardian-side only, never wire
       *  (see `domain/namedTimes.ts` and `Policy.freeGroups`). */
      freeGroups?: FreeGroupRecord[];
    },
  ) => Promise<ClauseDelivery>;
  setBlocked: (childId: string, policyId: string, blocked: boolean) => Promise<void>;
  /** App-scope: save an app's block + allowed-hours as ONE signed change. The
   *  whole per-app rule is replace-the-set (aggregated into the child's single
   *  `appRules` clause), so both dimensions travel together. `schedule`
   *  undefined = no per-app time limit. */
  saveAppRule: (
    childId: string,
    policyId: string,
    next: { blocked: boolean; schedule?: Schedule },
  ) => Promise<void>;
  addAppLimit: (childId: string, app: { appId: string; label: string }) => Policy;
  removePolicy: (childId: string, policyId: string) => void;

  // Approvals (parent decisions — signed when a signer is connected)
  approveRequest: (id: string, minutesGranted?: number) => Promise<void>;
  denyRequest: (id: string) => Promise<void>;
  /** Answer an `app.open` ask with one of its one-tap windows (an absolute
   *  unix-seconds instant, already resolved by the caller — see
   *  `wire/grant.ts`'s `appOpenWindowUnix` — plus which window it was, for
   *  the activity-log phrasing). Deny goes through `denyRequest` unchanged —
   *  no clause is signed for it. */
  approveAppOpen: (id: string, untilUnix: number, window: AppOpenWindow) => Promise<void>;
  /**
   * Let a repeat ask go without answering it (approvals-clarity, 2026-08-04).
   * Local-only: no clause, no signer call, no relay traffic, no change to any
   * ward state — works offline and with no signer connected. The ward is NOT
   * told: this leaves the request exactly as unanswered as it already was,
   * which is what a guardian simply not acting already looks like from her
   * side (her device still shows "asked" and lets her ask again once it goes
   * stale). Idempotent, and available on every request — not gated to "this
   * looks like a duplicate", so there is no rule to learn to use it.
   */
  dismissRequest: (id: string) => void;
  /** Persist the guardian's stepped-down grant amount on the request record
   *  itself (M-1) — so a failed send followed by a remount still shows what
   *  they chose, not the full amount back again. Local-only bookkeeping,
   *  nothing signed or published. */
  setRequestChosenMinutes: (id: string, minutes: number) => void;

  // Signer
  connectSigner: (kind: SignerKind) => Promise<void>;
  /** Turn on the local-key signer (this phone holds the guardian key). */
  enableLocalSigner: () => Promise<void>;
  /** Recovery: install a restored guardian secret; returns its pubkey (hex). */
  restoreGuardianKey: (secret: Uint8Array) => Promise<string>;
  /** Pair the real Signet bunker from a `bunker://…` URI. */
  pairSignet: (bunkerUri: string) => Promise<void>;
  disconnectSigner: () => Promise<void>;
  setAutoSign: (on: boolean) => Promise<void>;

  // Manual-signing confirm flow (surfaced as an "Approve in Signet" sheet)
  pendingSignature: PendingSignature | null;
  resolveSignature: (ok: boolean) => void;
}

const CharterContext = createContext<CharterContextValue | null>(null);

let idCounter = 0;
function uid(prefix: string): string {
  idCounter += 1;
  return `${prefix}_${Date.now().toString(36)}_${idCounter}`;
}

const CHILD_COLORS = ["#3B6FB2", "#2E7D5B", "#C4860F", "#7A4FB5", "#B23B30"];

export function CharterProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, undefined, loadState);
  const [pendingSignature, setPendingSignature] =
    useState<PendingSignature | null>(null);
  const resolverRef = useRef<((ok: boolean) => void) | null>(null);

  // Keep a ref to the latest state so async (post-await) actions read fresh data.
  const stateRef = useRef(state);
  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  // Persist on every change.
  useEffect(() => {
    persist(state);
  }, [state]);

  // The confirm gate drives the "Approve in Signet" sheet when auto-sign is off
  // (the mock signer). Stable so the signer can be rebuilt on (un)pair.
  const confirmGate = useRef<ConfirmGate>((ctx) =>
    new Promise<boolean>((resolve) => {
      resolverRef.current = resolve;
      setPendingSignature({
        action: ctx.action,
        decision: ctx.decision,
        signerLabel: ctx.signerLabel,
        signerKind: ctx.signerKind,
        detail: ctx.detail,
      });
    }),
  ).current;

  // Resolve a child id → the signer's delivery target, reading the LATEST
  // children (so a clause always targets the child's current devices).
  const resolveChild = useCallback(
    (childId: string) => resolveChildTarget(stateRef.current.children, childId, DEFAULT_RELAYS),
    [],
  );

  // The child's LATEST policy list — the signer aggregates every app-scope
  // policy into ONE `appRules` clause, so it needs the whole set (reading fresh
  // state, exactly like resolveChild).
  const childPolicies = useCallback(
    (childId: string): Policy[] =>
      stateRef.current.children.find((c) => c.id === childId)?.policies ?? [],
    [],
  );

  // Pick the signer from persisted state: kind "local" → the local-key
  // self-signer; a paired `bunkerUri` → the REAL Signet signer; otherwise the
  // local mock (the demo default). Rebuilt on (un)pair.
  const buildSigner = useCallback(
    (signerState: SignerState): Signer =>
      selectSigner(signerState, { resolveChild, childPolicies, confirmGate }),
    [resolveChild, childPolicies, confirmGate],
  );

  // Created once for the provider lifetime; reassigned by pair/disconnect.
  const signer = useRef<Signer | null>(null);
  if (signer.current === null) {
    signer.current = buildSigner(state.signer);
  }

  // On reload, a persisted local signer must re-open (loads the browser key — no
  // network). The Signet path reconnects on its own pairing action.
  useEffect(() => {
    if (state.signer.kind === "local" && !signer.current!.status().connected) {
      signer.current!
        .connect("local")
        .then(syncSigner)
        .catch(() => {
          dispatch({ type: "SET_SIGNER", signer: { connected: false, kind: "none", autoSign: stateRef.current.signer.autoSign } });
        });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const resolveSignature = useCallback((ok: boolean) => {
    const resolve = resolverRef.current;
    resolverRef.current = null;
    setPendingSignature(null);
    resolve?.(ok);
  }, []);

  const syncSigner = useCallback(() => {
    dispatch({ type: "SET_SIGNER", signer: signer.current!.status() });
  }, []);

  const addActivity = useCallback(
    (
      childId: string,
      outcome: ActivityOutcome,
      summary: string,
      // Named times (M-2): when this event is an extend/gift that credited
      // ONE group, record it structurally — `groupExtrasToday` sums these to
      // keep the Today card's displayed cap honest about what the guardian
      // themself granted, rather than parsing it back out of `summary`.
      grant?: { bucketId?: string; minutesGranted?: number },
    ) => {
      dispatch({
        type: "ADD_ACTIVITY",
        event: {
          id: uid("act"),
          childId,
          ts: Date.now(),
          outcome,
          summary,
          bucketId: grant?.bucketId,
          minutesGranted: grant?.minutesGranted,
        },
      });
    },
    [],
  );

  // --- Family actions ---------------------------------------------------

  const addChild = useCallback((name: string, color?: string): Child => {
    const child: Child = {
      id: uid("child"),
      name: name.trim() || "New child",
      color: color ?? CHILD_COLORS[idCounter % CHILD_COLORS.length],
      // Stage 0: a child is a local label — no Signet account.
      dependantPubkey: null,
      // Devices are added later via the scan/pair flow.
      devices: [],
      policies: [
        { id: uid("pol"), scope: { kind: "device" } },
      ],
    };
    dispatch({ type: "ADD_CHILD", child });
    return child;
  }, []);

  /** Edit a ward's identity: their name and the colour spot that stands for
   *  them across every screen. Guardian-side display only — neither travels
   *  on the wire, so nothing here needs signing. */
  const editChild = useCallback(
    (childId: string, name: string, color?: string) => {
      dispatch({
        type: "EDIT_CHILD",
        childId,
        name: name.trim() || "Child",
        color,
      });
    },
    [],
  );


  // --- Devices (parent-governed pairing) -------------------------------

  const addDevice = useCallback(
    (childId: string, label: string, platform: Device["platform"] = "linux"): Device => {
      const device: Device = {
        id: uid("dev"),
        label: label.trim() || "New device",
        platform,
        pairing: "unpaired",
        devicePubkey: null,
      };
      dispatch({ type: "ADD_DEVICE", childId, device });
      return device;
    },
    [],
  );

  const beginPairing = useCallback((childId: string, deviceId: string) => {
    dispatch({ type: "SET_DEVICE_PAIRING", childId, deviceId, pairing: "pairing" });
  }, []);

  /**
   * Adopt a phone we have been hearing from but never recorded — the other half
   * of a split-brain pairing (see ./unclaimedDevices).
   *
   * Takes the key straight from the STATUS we opened with the guardian's own
   * secret, so there is no code to re-type and no chance of a typo minting a
   * non-deliverable target. Re-checks the hex anyway: a bad recipient throws
   * inside the gift-wrap and would abort the whole sign, starving the child's
   * working devices too.
   *
   * And re-checks the PAIRING (S2). The caller's list was filtered on the same
   * evidence, but this is the function that binds a stranger's machine as a
   * child's phone and pushes their whole charter to it, so it does not take
   * the list's word for it: the machine must still be echoing a token this
   * guardian minted, right now, on its freshest heartbeat. A stale render, a
   * replayed tap, or a future caller that forgets the filter all land here and
   * are refused rather than trusted.
   */
  const claimDevice = useCallback(
    (childId: string, machine: string, label: string): Device | undefined => {
      if (!/^[0-9a-f]{64}$/.test(machine)) return undefined;
      // Never let the same phone be adopted twice, under this child or a sibling.
      const taken = stateRef.current.children.some((c) =>
        c.devices.some((d) => d.devicePubkey === machine),
      );
      if (taken) return undefined;
      const echoed = deviceStatusRef.current[machine]?.pairToken;
      if (!echoed || !livePairTokens().has(echoed)) return undefined;
      const device = addDevice(childId, label, "android");
      const pairedAt = Date.now();
      dispatch({
        type: "SET_DEVICE_PAIRING",
        childId,
        deviceId: device.id,
        pairing: "paired",
        pairedAt,
        devicePubkey: machine,
      });
      addActivity(childId, "enacted", `You set up ${device.label}`);
      // Spend the token: it has done the one job it existed for, and a spent
      // token left in the ledger could vouch for a second machine.
      forgetPairToken(echoed);
      // The phone has received NOTHING yet — park a resend of the child's
      // standing charter, released once the claim is committed (the device
      // is not in state until the next render; see ./resendOnClaim).
      pendingResends.current.push({ childId, machine });
      return { ...device, pairing: "paired", pairedAt, devicePubkey: machine };
    },
    [addDevice, addActivity],
  );

  // "Govern this device": bind the code shown on the device as the gift-wrap
  // recipient. The code MUST parse to a real key (hex / npub / nprofile) —
  // anything else is refused loudly (returns undefined, nothing stored). The
  // old silent `mockpub_…` fallback made an unparseable code look paired while
  // being a non-deliverable target — a field-pairing trap, not a feature.
  const confirmPairing = useCallback(
    (childId: string, device: Device, scannedCode: string): Device | undefined => {
      const devicePubkey = parseDevicePairingCode(scannedCode);
      if (!devicePubkey) return undefined;
      const pairedAt = Date.now();
      dispatch({
        type: "SET_DEVICE_PAIRING",
        childId,
        deviceId: device.id,
        pairing: "paired",
        pairedAt,
        devicePubkey,
      });
      // Build the result from the device we were handed — it was created this
      // same tick by addDevice, so it is NOT yet in committed state (stateRef
      // only refreshes post-render). Reading stateRef here would always miss it
      // and make a real pairing look like a failure (and invite a duplicate on
      // retry). Success is determined solely by the code parsing above.
      addActivity(childId, "enacted", `You connected ${device.label}`);
      return { ...device, pairing: "paired", pairedAt, devicePubkey };
    },
    [addActivity],
  );

  // Re-connect a device that already handed us its real key — reuses the stored
  // devicePubkey, never mints a new one (the old "rescan" path clobbered a real
  // key with a mock). Returns undefined when there is no real key to reuse; the
  // UI then routes through the full set-up flow instead.
  const reconnectDevice = useCallback(
    (childId: string, deviceId: string): Device | undefined => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const device = child?.devices.find((d) => d.id === deviceId);
      const key = device?.devicePubkey;
      if (!device || !key || !/^[0-9a-f]{64}$/.test(key)) return undefined;
      const pairedAt = Date.now();
      dispatch({
        type: "SET_DEVICE_PAIRING",
        childId,
        deviceId,
        pairing: "paired",
        pairedAt,
        devicePubkey: key,
      });
      addActivity(childId, "enacted", `You reconnected ${device.label}`);
      return { ...device, pairing: "paired", pairedAt };
    },
    [addActivity],
  );

  // Demo/seed convenience only: mints an explicitly-mock, non-deliverable
  // pairing. Production pairing goes through confirmPairing with a real code.
  const enrollDevice = useCallback(
    (childId: string, label: string): Device => {
      const device = addDevice(childId, label);
      const pairedAt = Date.now();
      const devicePubkey = `mockpub_${Math.random().toString(36).slice(2, 10)}`;
      dispatch({
        type: "SET_DEVICE_PAIRING",
        childId,
        deviceId: device.id,
        pairing: "paired",
        pairedAt,
        devicePubkey,
      });
      return { ...device, pairing: "paired", pairedAt, devicePubkey };
    },
    [addDevice],
  );

  // Publish a signed RELEASE to a device (best-effort) so the phone actually
  // un-governs itself — drops its pairing, forgets its clauses, lifts every
  // restriction. Parent-gated by construction (only the guardian key signs it).
  const releaseDeviceOverWire = useCallback((childId: string, deviceId: string) => {
    const child = stateRef.current.children.find((c) => c.id === childId);
    const device = child?.devices.find((d) => d.id === deviceId);
    const pk = device?.devicePubkey;
    if (pk && /^[0-9a-f]{64}$/.test(pk)) {
      // Fire-and-forget: the UI updates immediately; the relay carries the
      // release, and the phone honors it on its next poll.
      // Pass the device's own name so the confirm sheet can say what is being
      // disconnected rather than showing a key prefix (S9).
      void signer.current?.releaseDevice(pk, DEFAULT_RELAYS, device?.label).catch(() => {});
    }
  }, []);

  const removeChild = useCallback((childId: string) => {
    // RELEASE every paired device FIRST. Removing the child deletes the only
    // record of each `devicePubkey` — and with it any ability to ever sign a
    // release, a grant or a new clause to that device. A Device-Owner phone
    // left behind keeps enforcing its last charter forever.
    const child = stateRef.current.children.find((c) => c.id === childId);
    for (const deviceId of devicesToRelease(child)) {
      releaseDeviceOverWire(childId, deviceId);
    }
    dispatch({ type: "REMOVE_CHILD", childId });
  }, [releaseDeviceOverWire]);

  // Scan-to-pair: after the camera reads a ward's QR, tell that ward who we
  // are. It has no other way to learn our key — scanning teaches this phone
  // the laptop's address, never the reverse.
  const sendPairOffer = useCallback(async (machine: string, token: string) => {
    await signer.current?.sendPairOffer(machine, token, DEFAULT_RELAYS);
  }, []);

  const unpairDevice = useCallback(
    (childId: string, deviceId: string) => {
      releaseDeviceOverWire(childId, deviceId);
      dispatch({
        type: "SET_DEVICE_PAIRING",
        childId,
        deviceId,
        pairing: "unpaired",
        devicePubkey: null,
      });
      const child = stateRef.current.children.find((c) => c.id === childId);
      const label =
        child?.devices.find((d) => d.id === deviceId)?.label ?? "the device";
      addActivity(childId, "rule-changed", `You disconnected ${label}`);
    },
    [addActivity, releaseDeviceOverWire],
  );

  const removeDevice = useCallback(
    (childId: string, deviceId: string) => {
      releaseDeviceOverWire(childId, deviceId);
      dispatch({ type: "REMOVE_DEVICE", childId, deviceId });
    },
    [releaseDeviceOverWire],
  );

  // --- Stage 1 intake (mock a device-originated ask) -------------------

  const simulateDeviceRequest = useCallback(
    (
      childId: string,
      deviceId: string,
      kind: RequestKind,
      opts?: {
        appLabel?: string;
        appId?: string;
        minutesRequested?: number;
        reason?: string;
        title?: string;
        limitHit?: "schedule" | "budget";
      },
    ): ChildRequest => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const deviceLabel =
        child?.devices.find((d) => d.id === deviceId)?.label ?? "This device";
      const defaultTitle =
        kind === "time.extend"
          ? `${deviceLabel} asks for ${opts?.minutesRequested ?? 15} more minutes`
          : kind === "install.app"
            ? `${deviceLabel} asks to install ${opts?.appLabel ?? "an app"}`
            : `${deviceLabel} asks to run ${opts?.appLabel ?? "a program"}`;
      const request: ChildRequest = {
        id: uid("req"),
        childId,
        requester: "device",
        deviceId,
        kind,
        createdAt: Date.now(),
        title: opts?.title ?? defaultTitle,
        reason: opts?.reason,
        minutesRequested: opts?.minutesRequested,
        limitHit: opts?.limitHit,
        appLabel: opts?.appLabel,
        appId: opts?.appId,
        status: "pending",
      };
      dispatch({ type: "ADD_REQUEST", request });
      return request;
    },
    [],
  );

  // --- Live intake (real device-originated asks over the relay) --------

  // Fold one unwrapped REQUEST (kind 31111) into the queue — the real sibling
  // of simulateDeviceRequest. Dedupes by reqId against the persisted queue
  // (relays re-deliver across reloads; the subscription itself dedupes within
  // a session) and drops asks from machines no paired device claims — an
  // ungoverned device has no standing to ask.
  /**
   * Take in a ward's ask. Returns whether it was actually recorded — false when
   * there is nobody to attribute it to, so the caller does not mark it seen and
   * a later poll can pick it up again.
   */
  const ingestDeviceRequest = useCallback((r: DeviceRequest): boolean => {
    const s = stateRef.current;
    if (s.requests.some((q) => q.reqId === r.reqId)) return true;
    if (r.op === "install.apk") {
      return ingestInstallAsk(r);
    }
    let childId: string | undefined;
    let device: Device | undefined;
    for (const c of s.children) {
      const d = c.devices.find(
        (dev) => dev.pairing === "paired" && dev.devicePubkey === r.machine,
      );
      if (d) {
        childId = c.id;
        device = d;
        break;
      }
    }
    // Not attributable YET — the phone is paired on its own side but missing
    // from the guardian's records. Say so by returning false; do NOT consume it.
    if (!childId || !device) return false;
    // app.open (named times' "ask to open") answers via a plain Decision echo
    // + a re-signed `apps` clause carrying an AppHold — see `approveAppOpen`
    // and `wire/grant.ts`'s `buildAppOpenGrant`, a different shape than the
    // time.extend ask below.
    const request =
      r.op === "app.open"
        ? appOpenRequestToCard(r, device, childId, uid("req"), Date.now())
        : timeExtendRequestToCard(r, device, childId, uid("req"), Date.now());
    dispatch({ type: "ADD_REQUEST", request });
    return true;
  }, []);

  // A phone asking to install an app ("this app, this once" — the Approvals
  // card renders it with single-use semantics). Approving is LOCAL-ONLY until
  // the signer can pin the APK's cert digest (the grant REQUIRES
  // signerCertSha256; sourcing it is the install-enactment increment).
  const ingestInstallAsk = useCallback((r: DeviceRequest & { op: "install.apk" }): boolean => {
    const s = stateRef.current;
    let childId: string | undefined;
    let device: Device | undefined;
    for (const c of s.children) {
      const d = c.devices.find(
        (dev) => dev.pairing === "paired" && dev.devicePubkey === r.machine,
      );
      if (d) {
        childId = c.id;
        device = d;
        break;
      }
    }
    if (!childId || !device) return false; // no standing YET — retry, don't drop
    const label = r.params.label ?? r.params.packageName;
    dispatch({
      type: "ADD_REQUEST",
      request: {
        id: uid("req"),
        childId,
        requester: "device",
        deviceId: device.id,
        kind: "install.app",
        createdAt: Math.min(r.ts * 1000, Date.now()),
        title: `${device.label} asks to install ${label}`,
        appLabel: label,
        appId: r.params.packageName,
        status: "pending",
        reqId: r.reqId,
        nonce: r.nonce,
        machine: r.machine,
        subject: r.subject,
      },
    });
    return true;
  }, []);

  // QR onboarding: while the parent is showing a pairing QR, this holds the
  // outstanding one-time token; the phone that scans it echoes the token on
  // its STATUS heartbeat, and `foundMachine` fills in with the device's real
  // pubkey — no one types a device code. Ephemeral by design (dies with the
  // session; the token also expires device-side).
  const [phonePairing, setPhonePairing] = useState<{
    childId: string;
    token: string;
    foundMachine: string | null;
  } | null>(null);
  const phonePairingRef = useRef(phonePairing);
  phonePairingRef.current = phonePairing;

  const beginPhonePairing = useCallback((childId: string): string => {
    const token = mintPairToken();
    // Durably, not just in this session's state (S2). The in-session copy
    // drives the live "waiting for the phone" match; the ledger is what lets
    // the split-brain recovery still recognise this phone after the app was
    // closed and reopened — which is the only situation that recovery is for.
    rememberPairToken(token, childId);
    setPhonePairing({ childId, token, foundMachine: null });
    return token;
  }, []);

  const cancelPhonePairing = useCallback(() => setPhonePairing(null), []);

  // The device STATUS heartbeat (kind 31114) — each valid feed stamps the
  // matching device's `lastSeenAt`. Its ABSENCE is the signal: a wiped or
  // escaped device simply goes dark, and "last seen" is how the guardian
  // notices (the audit-gap surface). Same local-key-only decrypt seam as the
  // REQUEST intake above. A heartbeat echoing the outstanding pairing token
  // resolves the QR-onboarding wait with the device's machine pubkey.
  const [deviceStatus, setDeviceStatus] = useState<Record<string, DeviceStatus>>({});
  // Mirrors deviceStatus for the []-dep callbacks (same pattern as stateRef):
  // giveTime needs the freshest reading to record the ward's standing.
  const deviceStatusRef = useRef(deviceStatus);
  useEffect(() => {
    deviceStatusRef.current = deviceStatus;
  }, [deviceStatus]);

  // Phones we are hearing from that nobody's device list references. Derived,
  // not stored: the moment one is claimed it drops out of here by itself, and
  // one that stops beating (a RELEASED phone honoring its release) ages out on
  // the next recompute rather than being re-offered for adoption.
  const unclaimed = useMemo(
    () => unclaimedDevices(state.children, deviceStatus, livePairTokens(), Date.now()),
    [state.children, deviceStatus],
  );

  // Per-device, per-day usage history (design memo B1). Its OWN localStorage key
  // (not the state blob, not the household backup): it's observational, high-
  // churn, and regenerates. Pruned to ~60 days on load so it can't grow forever.
  const [usageHistory, setUsageHistory] = useState<UsageHistory>(() => {
    try {
      const raw = localStorage.getItem(USAGE_KEY);
      const parsed = raw ? (JSON.parse(raw) as UsageHistory) : emptyUsageHistory;
      return parsed && parsed.byMachine ? pruneHistory(parsed, Date.now()) : emptyUsageHistory;
    } catch {
      return emptyUsageHistory;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(USAGE_KEY, JSON.stringify(usageHistory));
    } catch {
      /* storage full / blocked — the weekly picture just won't persist */
    }
  }, [usageHistory]);

  // B3 aggregator: whenever the usage history moves, send each device of a
  // multi-device child its "elsewhere" view (USAGE_SYNC 31115) so the wardens
  // can enforce the POOLED budget. Change-gated per device (identical view →
  // no publish) with a 60s floor; ts strictly monotonic per subject so a
  // warden's replay rule never rejects a legitimate consecutive sync. Local
  // signer only — the same guard as the STATUS intake poll below.
  const lastSyncSentRef = useRef<Record<string, { key: string; at: number }>>({});
  const lastSyncTsRef = useRef<Record<string, number>>({});
  useEffect(() => {
    const s = signer.current;
    if (!s?.publishUsageSyncs || stateRef.current.signer.kind !== "local") return;
    const nowMs = Date.now();
    for (const child of stateRef.current.children) {
      const devices = child.devices.filter(
        (d) => d.pairing === "paired" && /^[0-9a-f]{64}$/.test(d.devicePubkey ?? ""),
      );
      if (devices.length < 2) continue;
      const subject = child.dependantPubkey;
      if (!subject) continue;

      const payloads: Record<string, import("../wire/usageSync").UsageSyncPayload> = {};
      for (const receiver of devices) {
        const machine = receiver.devicePubkey as string;
        // The day the RECEIVER is currently in (its freshest STATUS); without
        // one we can't name the day honestly, so skip it this round.
        const dayKey = deviceStatus[machine]?.dayKey;
        if (!dayKey) continue;
        const view = buildUsageSyncForDevice({
          subject,
          receiverMachine: machine,
          dayKey,
          ts: 0, // stamped below, outside the change-key
          devices: devices.map((d) => {
            const m = d.devicePubkey as string;
            return {
              machine: m,
              secs: usageHistory.byMachine[m]?.[dayKey] ?? 0,
              minutesB64: usageHistory.minutesByMachine?.[m]?.[dayKey],
            };
          }),
        });
        if (view.spentElsewhereTodaySecs === 0 && !view.elsewhereMinutesToday) continue;
        const key = JSON.stringify({ ...view, ts: 0 });
        const last = lastSyncSentRef.current[machine];
        if (last && (last.key === key || nowMs - last.at < 60_000)) continue;
        const ts = Math.max(Math.floor(nowMs / 1000), (lastSyncTsRef.current[subject] ?? 0) + 1);
        lastSyncTsRef.current[subject] = ts;
        lastSyncSentRef.current[machine] = { key, at: nowMs };
        payloads[machine] = { ...view, ts };
      }
      if (Object.keys(payloads).length > 0) {
        // Fire-and-forget: a failed relay publish just retries on the next
        // history change (the change-gate key was recorded; the 60s floor
        // passes and an identical view won't resend — acceptable: the NEXT
        // real usage change re-triggers, and staleness only under-counts).
        void s.publishUsageSyncs(child.id, payloads).catch(() => {});
      }
    }
  }, [usageHistory, deviceStatus]);

  // The published Kintrinsic artifact manifest (#44) — one fetch per session,
  // fail-quiet: no manifest simply means no update to offer.
  const [updateManifest, setUpdateManifest] = useState<UpdateManifest | null>(null);
  const [debManifest, setDebManifest] = useState<DebManifest | null>(null);

  /** Re-read the published artifact manifests. Mounted-once was a trap: a
   *  release published while this app sat open was invisible until the guardian
   *  closed and reopened it, so the Update button kept offering the version
   *  from whenever the app happened to start (hit twice on 2026-07-26, two
   *  releases in one session). Now also re-read on focus + pull-to-refresh.
   *
   *  D2: the primary source is the signed relay announcement (verified against
   *  the pinned release key); the origin JSON feeds are the unsigned fallback
   *  until the fleet is proven on relays (then D3 removes them). */
  const refetchManifests = useCallback(async () => {
    const [relay, apk, deb] = await Promise.all([
      fetchAllReleaseManifests().catch(() => ({ ward: null, carrier: null, deb: null })),
      fetchManifest().catch(() => null),
      // The Linux artifact, so a paired laptop can be told it's behind too.
      fetchDebManifest().catch(() => null),
    ]);
    const ward = relay.ward ?? apk;
    const linux = relay.deb ? { ...relay.deb, path: "" } : deb;
    if (ward) setUpdateManifest(ward);
    if (linux) setDebManifest(linux);
  }, []);

  useEffect(() => {
    void refetchManifests();
  }, [refetchManifests]);

  /**
   * Give a child minutes without them asking — and after a "no" you've thought
   * better of. Rides a signed `gift` clause rather than the grant path, since a
   * GRANT must echo a pending request; before this the only lever was editing
   * the schedule, which rewrites a standing rule to solve a one-off.
   */
  const giveTime = useCallback(
    async (childId: string, minutes: number, groupId?: string): Promise<boolean> => {
      const s = signer.current;
      if (!s?.signGiftClause || !s.status().connected) return false;
      const child = stateRef.current.children.find((c) => c.id === childId);
      const devicePolicy = child?.policies.find((p) => p.scope.kind === "device");
      // The child's own day boundary, never the guardian phone's — schedule,
      // then budget, then (a named-times-only ward's only tz) buckets (C-1).
      const tz = resolveChildTz(devicePolicy);
      try {
        await s.signGiftClause(childId, minutes, tz, groupId);
        // Record the standing alongside the gift. "You gave 5 more minutes"
        // reads the same whether the ward was on target or two hours over;
        // saying which makes the pattern visible across a week instead of
        // only in the moment it was granted.
        const note = child
          ? standingNote(standingFor(child, deviceStatusRef.current))
          : null;
        const groupLabel = groupId
          ? devicePolicy?.buckets?.buckets.find((b) => b.id === groupId)?.label
          : undefined;
        const what = groupLabel
          ? `You gave ${minutes} more minutes to ${groupLabel}`
          : `You gave ${minutes} more minutes`;
        // Named times (M-2): a gift straight to a group credits its cap the
        // same way an approved bucket extend does — record it structurally.
        addActivity(
          childId,
          "rule-changed",
          note ? `${what} (${note})` : what,
          groupId ? { bucketId: groupId, minutesGranted: minutes } : undefined,
        );
        return true;
      } catch (err) {
        if (err instanceof SignerCancelled) return false;
        throw err;
      }
    },
    [addActivity],
  );

  /**
   * Call a stand-down ("finish up now"), or lift one. The inverse of
   * [giveTime], and gated the same way: nothing is claimed unless something was
   * signed.
   */
  const standDown = useCallback(
    async (childId: string, lift = false): Promise<boolean> => {
      const s = signer.current;
      if (!s?.signStandDownClause || !s.status().connected) return false;
      const child = stateRef.current.children.find((c) => c.id === childId);
      const devicePolicy = child?.policies.find((p) => p.scope.kind === "device");
      // The ward's own day boundary decides when it lapses, never the
      // guardian phone's — the same rule a gift's expiry follows (C-1).
      const tz = resolveChildTz(devicePolicy);
      try {
        await s.signStandDownClause(childId, { lift, tz });
        addActivity(
          childId,
          "rule-changed",
          lift ? "You allowed them back on" : "You asked them to finish up now",
        );
        return true;
      } catch (err) {
        if (err instanceof SignerCancelled) return false;
        throw err;
      }
    },
    [addActivity],
  );

  const openMaintenanceWindow = useCallback(
    async (childId: string, minutes: number): Promise<boolean> => {
      const s = signer.current;
      if (!s?.signMaintenanceClause || !s.status().connected) return false;
      try {
        await s.signMaintenanceClause(childId, minutes);
        addActivity(
          childId,
          "rule-changed",
          minutes <= 0
            ? "You closed the install window"
            : `You allowed installs for ${minutes} minutes`,
        );
        return true;
      } catch (err) {
        if (err instanceof SignerCancelled) return false;
        throw err;
      }
    },
    [addActivity],
  );

  const sendCharterUpdate = useCallback(
    async (childId: string): Promise<boolean> => {
      const manifest = updateManifest;
      if (!manifest || !signer.current!.status().connected) return false;
      // A relay-announced release names absolute Blossom mirrors — the clause
      // carries ONE, so pick the canonical address, never a CDN redirect
      // target (the 0.6.9 event led with one; it 404'd for every ward,
      // 2026-08-27). The origin JSON fallback names a single Blossom `url`
      // (D3); only very old manifests fall back to a same-origin path. Either
      // way the ward re-verifies the bytes against the clause's sha256 + cert pins.
      const mirrors = (manifest as Partial<ReleaseManifest>).urls;
      const url =
        (mirrors && pickInstallUrl(mirrors, manifest.apkSha256)) ??
        (manifest.url
          ? manifest.url
          : new URL(manifest.path ?? APK_PATH, window.location.origin).toString());
      try {
        await signer.current!.signUpdateClause(childId, manifest, url);
        addActivity(
          childId,
          "rule-changed",
          `You sent a Kintrinsic update (${manifest.versionName}) to the devices`,
        );
        return true;
      } catch (err) {
        if (err instanceof SignerCancelled) return false;
        throw err;
      }
    },
    [updateManifest, addActivity],
  );

  const ingestDeviceStatus = useCallback((status: DeviceStatus) => {
    dispatch({
      type: "SET_DEVICE_LAST_SEEN",
      machine: status.machine,
      lastSeenAt: status.ts * 1000,
    });
    // Record the day's usage for the weekly picture (B1) + the device's minute
    // journal (B3 union rule). recordUsage keeps the max seen / unions the
    // journal and returns the same ref on a no-op, so repeat polls don't churn.
    setUsageHistory((h) =>
      recordUsage(h, status.machine, status.dayKey, status.usedTodaySecs, status.activeMinutesToday),
    );
    // Keep the freshest live report per machine — screens prefer the device's
    // REAL state over the local guess while it's fresh (liveStatusFor).
    setDeviceStatus((prev) => {
      const cur = prev[status.machine];
      return cur && cur.ts >= status.ts ? prev : { ...prev, [status.machine]: status };
    });
    const pending = phonePairingRef.current;
    if (pending && status.pairToken === pending.token && !pending.foundMachine) {
      setPhonePairing({ ...pending, foundMachine: status.machine });
    }
  }, []);

  // POLLED, not pushed: a long-lived relay subscription dies silently in a
  // browser (proxies idle-drop websockets after ~60s and nostr-tools does not
  // resubscribe on reconnect — caught live when a paired phone's heartbeats
  // stopped arriving mid-session). A short fresh query per tick is immune and
  // idempotent: lastSeenAt is monotonic, the token match fires once, and
  // REQUESTs dedupe by reqId (in-session set + the persisted queue). ONE
  // query feeds both intakes — STATUS heartbeats and the children's asks.
  // Fast cadence while a pairing QR is on screen — the parent is watching.
  const seenReqIds = useRef<Set<string>>(new Set());
  const phonePairingWaiting = !!(phonePairing && !phonePairing.foundMachine);

  /**
   * How far back the next read asks for, and which wraps it can skip.
   *
   * Re-reading the whole day every 30s was described here as free because the
   * ingests dedupe. The ingests do — the DECRYPTION does not. Every tick pulled
   * the full 24h of gift-wraps down again and ran a NIP-44 unwrap on each one
   * before anything could recognise it as old news, so a guardian with a busy
   * day paid hundreds of decryptions a minute, forever, to learn nothing. On a
   * phone that is the app's whole battery cost (decented, 2026-08-01).
   *
   * So: a cursor for the wire, and a handled-set for the CPU. The 24h floor
   * still applies, which keeps the property that made the wide window worth
   * having — a wrap that could not be ingested yet (an ask from a phone this
   * guardian hasn't added) is left unhandled, holds the cursor where it is, and
   * is retried every tick until it lands or ages out. The first read of a
   * session has no cursor and sweeps the full day, so a reload always heals.
   */
  const pollCursor = useRef<number | null>(null);
  const handledWraps = useRef<Set<string>>(new Set());

  /** One relay read: STATUS heartbeats + the children's asks. Hoisted out of
   *  the interval so a focus event and a pull-to-refresh can run exactly the
   *  same fetch the timer does. */
  const pollRelayOnce = useCallback(async () => {
    if (stateRef.current.signer.kind !== "local") return;
    let guardianSk: Uint8Array;
    try {
      guardianSk = loadOrCreateGuardianKey();
    } catch {
      return;
    }
    const guardianPk = getPublicKey(guardianSk);
    const pool = new SimplePool();
    // Stamped BEFORE the read, so anything published while it is in flight is
    // still inside the next window rather than falling down the gap.
    const startedAt = Math.floor(Date.now() / 1000);
    // Asks stay approvable for hours (the child may be waiting at the lock
    // screen while the parent's phone was in a pocket), so the day-wide floor
    // stands — the cursor only ever narrows the window WITHIN it.
    const since = windowSince(pollCursor.current, startedAt);
    // Set by anything this round could not finish with. It pins the cursor, so
    // the wide window keeps coming back until the wrap lands or ages out.
    let unfinished = false;
    try {
      const wraps = await pool.querySync(DEFAULT_RELAYS, {
        kinds: [1059],
        "#p": [guardianPk],
        since,
      });
      for (const w of wraps) {
        // The one line that saves the battery: everything below this decrypts.
        if (handledWraps.current.has(w.id)) continue;
        const s = unwrapStatus(w, guardianSk);
        if (s) {
          ingestDeviceStatus(s);
          handledWraps.current.add(w.id);
          continue;
        }
        const req = unwrapRequest(w, guardianSk);
        if (!req) {
          // Not ours, or not readable. Neither will change with time.
          handledWraps.current.add(w.id);
          continue;
        }
        if (
          seenReqIds.current.has(req.reqId) ||
          stateRef.current.requests.some((r) => r.reqId === req.reqId)
        ) {
          handledWraps.current.add(w.id);
          continue;
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
        if (ingestDeviceRequest(req)) {
          seenReqIds.current.add(req.reqId);
          handledWraps.current.add(w.id);
        } else {
          unfinished = true;
        }
      }
      pollCursor.current = nextCursor(pollCursor.current, startedAt, unfinished);
      trimHandled(handledWraps.current);
    } catch {
      // Offline / relay down — routine; the next tick retries. The cursor is
      // deliberately left where it was: a failed read has covered nothing.
    } finally {
      pool.close(DEFAULT_RELAYS);
    }
  }, [ingestDeviceStatus, ingestDeviceRequest]);

  useEffect(() => {
    if (state.signer.kind !== "local") return;
    let stopped = false;
    const tick = () => {
      if (stopped) return;
      // A hidden app has nobody reading the screen it would update, and the
      // notification that actually matters comes from the carrier's own socket
      // — not from here. Most platforms freeze a backgrounded timer for us, but
      // "most" is not all: a desktop tab behind another window, and an Android
      // WebView, both keep firing. Skipping is what makes that free.
      if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
      void pollRelayOnce();
    };
    tick();
    const id = setInterval(tick, phonePairingWaiting ? 5_000 : 30_000);
    return () => {
      stopped = true;
      clearInterval(id);
    };
  }, [state.signer.kind, pollRelayOnce, phonePairingWaiting]);

  /** Everything a guardian expects a "refresh" to cover: what the devices have
   *  reported, and what versions the site is publishing. */
  const [refreshing, setRefreshing] = useState(false);
  const [refreshNotice, setRefreshNotice] = useState<string | null>(null);
  // Which pull owns the header. A late timeout from an abandoned refresh must
  // not clobber the state of the one the guardian just started.
  const refreshSeq = useRef(0);
  const refreshNow = useCallback(async () => {
    const seq = ++refreshSeq.current;
    setRefreshing(true);
    setRefreshNotice(null);
    const outcome = await raceRefresh(
      Promise.all([pollRelayOnce(), refetchManifests()]),
    );
    if (refreshSeq.current !== seq) return;
    setRefreshing(false);
    if (outcome !== "done") {
      // "Checking…" that never ends taught decented nothing in airplane mode
      // (2026-08-10): end it, and say so. One line for both causes — the app
      // cannot tell a downed relay from no connection, so it doesn't pretend.
      setRefreshNotice(REFRESH_OFFLINE_NOTICE);
      setTimeout(() => {
        if (refreshSeq.current === seq) setRefreshNotice(null);
      }, 6_000);
    }
  }, [pollRelayOnce, refetchManifests]);

  /**
   * Refresh when the app comes back to the foreground. A backgrounded PWA has
   * its timers frozen by the OS, so the 30s poll simply does not run while the
   * guardian is elsewhere — and on returning they were looking at whatever was
   * true when they left. This is what made "close and reopen it" the only way
   * to see a device's new version (decented, 2026-07-26); pull-to-refresh is the
   * manual version of the same fetch, not a substitute for it.
   */
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") void refreshNow();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [refreshNow]);

  // --- Signed rule changes ---------------------------------------------

  /**
   * Sign a policy change when it can actually be delivered, and say WHICH of
   * those happened.
   *
   * A draft is a real feature — rules legitimately predate the phone (write the
   * charter, then pair) — but this used to return a bare `true` for "signed and
   * sent" AND for "saved here, nothing left the building", so every caller
   * logged "You updated the daily time limit" either way. decented set
   * Mia's limit to 15 minutes on 2026-07-27 against a ward whose phone
   * Kintrinsic held no device record for: it saved, it read as done, and the
   * phone never heard a word (`clausesSeen=0`). A rule that never travelled
   * must not look identical to one that did — transparency is the invariant, so
   * the ward's phone and the guardian's screen have to agree.
   *
   * Undeliverable is decided BEFORE signing, so nobody is asked to approve a
   * clause that has nowhere to go.
   */
  const signClauseIfConnected = useCallback(
    async (childId: string, policy: Policy): Promise<ClauseDelivery> => {
      const blocked = clauseDeliveryPrecheck(
        signer.current!.status().connected,
        resolveChild(childId)?.devices.length ?? 0,
      );
      if (blocked) return blocked;
      try {
        await signer.current!.signClause(childId, policy);
        return { outcome: "sent" };
      } catch (err) {
        if (err instanceof SignerCancelled) return { outcome: "cancelled" };
        throw err;
      }
    },
    [resolveChild],
  );

  // Close the claim loop: a just-claimed phone has never been a target of any
  // rule save, so the guardian's standing charter goes to it by itself the
  // moment the claim is committed — instead of an instruction to re-save.
  const pendingResends = useRef<PendingResend[]>([]);
  useEffect(() => {
    const ready = readyResends(pendingResends.current, state.children);
    if (ready.length === 0) return;
    pendingResends.current = pendingResends.current.filter((p) => !ready.includes(p));
    for (const p of ready) {
      const policies =
        state.children.find((c) => c.id === p.childId)?.policies ?? [];
      if (policies.length === 0) continue; // nothing to send is not a failure
      void (async () => {
        let allSent = true;
        for (const policy of policies) {
          try {
            const d = await signClauseIfConnected(p.childId, policy);
            if (d.outcome !== "sent") allSent = false;
          } catch {
            allSent = false;
          }
        }
        addActivity(
          p.childId,
          "rule-changed",
          allSent
            ? "Their rules went to the new phone"
            : "Their rules could not all reach the new phone — save them again to retry",
        );
      })();
    }
  }, [state.children, signClauseIfConnected, addActivity]);

  const setSchedule = useCallback(
    async (childId: string, scopeId: string, schedule: Schedule) => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const existing = child?.policies.find((p) => p.id === scopeId);
      const policy: Policy = {
        id: scopeId,
        scope: existing?.scope ?? { kind: "device" },
        schedule,
        budget: existing?.budget,
      };
      const delivery = await signClauseIfConnected(childId, policy);
      if (delivery.outcome === "cancelled") return;
      dispatch({ type: "SET_POLICY", childId, policy });
      addActivity(
        childId,
        "rule-changed",
        "You updated the daily schedule" + undeliveredNote(delivery),
      );
    },
    [signClauseIfConnected, addActivity],
  );

  const setBudget = useCallback(
    async (childId: string, scopeId: string, budget: Budget) => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const existing = child?.policies.find((p) => p.id === scopeId);
      const policy: Policy = {
        id: scopeId,
        scope: existing?.scope ?? { kind: "device" },
        schedule: existing?.schedule,
        budget,
      };
      const delivery = await signClauseIfConnected(childId, policy);
      if (delivery.outcome === "cancelled") return;
      dispatch({ type: "SET_POLICY", childId, policy });
      addActivity(
        childId,
        "rule-changed",
        "You updated the daily time limit" + undeliveredNote(delivery),
      );
    },
    [signClauseIfConnected, addActivity],
  );

  // Save schedule and/or budget for one scope as a SINGLE signed change. Only the
  // dimensions actually passed are signed, and they share ONE issuedAt — so
  // saving both at once never re-signs a STALE copy of the other dimension (which
  // a fresh issuedAt would then make supersede the just-saved one on the device:
  // the dual-dimension save-revert bug). Local state keeps both dimensions.
  const savePolicy = useCallback(
    async (
      childId: string,
      scopeId: string,
      next: {
        schedule?: Schedule;
        budget?: Budget;
        web?: WebPolicy;
        apps?: AppsPolicy;
        learning?: LearningPolicy;
        tethering?: Tethering;
        lifeline?: Lifeline;
        buckets?: BucketsPolicy;
        listening?: ListeningPolicy;
        alwaysAvailable?: AlwaysAvailablePolicy;
        deviceOverrides?: Record<string, PolicyOverride>;
        freeGroups?: FreeGroupRecord[];
      },
    ): Promise<ClauseDelivery> => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const existing = child?.policies.find((p) => p.id === scopeId);
      const scope = existing?.scope ?? { kind: "device" as const };
      // Sign ONLY the changed dimension(s) — undefined ones emit no clause.
      const toSign: Policy = {
        id: scopeId,
        scope,
        schedule: next.schedule,
        budget: next.budget,
        web: next.web,
        apps: next.apps,
        learning: next.learning,
        tethering: next.tethering,
        lifeline: next.lifeline,
        buckets: next.buckets,
        listening: next.listening,
        alwaysAvailable: next.alwaysAvailable,
        // ONLY the overrides for dimensions actually being signed. The full map
        // would graft an unchanged control back on and emit a stale clause for
        // it (see pickOverrides). "learning" stays IN this list even though no
        // editor creates a new per-device learning split any more (named times
        // owns the whole dimension and is never split): a REVIEW ROUND ONE fix
        // tried excluding it to avoid a needless re-sign of a legacy override,
        // but `signClause` sends each device `effectivePolicyForDevice(toSign,
        // device.id)` — with "learning" missing from `toSign.deviceOverrides`,
        // a device that legitimately splits learning stopped receiving its OWN
        // rule and silently received the BASE clause instead (a real learning
        // rule replaced by whatever the shared value happened to be, the
        // moment `next.learning` was set — which is nearly every named-times
        // save). A cosmetic fresh-issuedAt re-sign of an untouched override is
        // an acceptable cost; silently overwriting a split device's actual
        // rule is not. See `effectivePolicy.test.ts` for the composition proof.
        deviceOverrides: pickOverrides(
          next.deviceOverrides,
          SPLITTABLE_CONTROLS.filter((c) => next[c] !== undefined),
        ),
        // Guardian-side only (see `Policy.freeGroups`) — carried along for
        // the same reason `deviceOverrides` is: `policyToClauses` never reads
        // it, so it rides the sign call inertly and never reaches the wire.
        freeGroups: next.freeGroups,
      };
      const delivery = await signClauseIfConnected(childId, toSign);
      if (delivery.outcome === "cancelled") return delivery;
      // Local state carries ALL dimensions (unchanged ones from the existing policy).
      const merged: Policy = {
        id: scopeId,
        scope,
        schedule: next.schedule ?? existing?.schedule,
        budget: next.budget ?? existing?.budget,
        web: next.web ?? existing?.web,
        apps: next.apps ?? existing?.apps,
        learning: next.learning ?? existing?.learning,
        tethering: next.tethering ?? existing?.tethering,
        lifeline: next.lifeline ?? existing?.lifeline,
        buckets: next.buckets ?? existing?.buckets,
        listening: next.listening ?? existing?.listening,
        alwaysAvailable: next.alwaysAvailable ?? existing?.alwaysAvailable,
        // Local state keeps the WHOLE map — it is the parent's editing truth,
        // not a wire artifact, and unsigned controls stay split.
        deviceOverrides: next.deviceOverrides ?? existing?.deviceOverrides,
        freeGroups: next.freeGroups ?? existing?.freeGroups,
      };
      dispatch({ type: "SET_POLICY", childId, policy: merged });
      const onlyApps =
        next.apps && !next.schedule && !next.budget && !next.web && !next.learning;
      const onlyWeb =
        next.web && !next.schedule && !next.budget && !next.apps && !next.learning;
      const onlyLearning =
        next.learning && !next.schedule && !next.budget && !next.web && !next.apps;
      addActivity(
        childId,
        "rule-changed",
        (onlyApps
          ? "You updated the app rules"
          : onlyWeb
            ? "You updated the website rules"
            : onlyLearning
              ? "You updated the learning apps"
              : "You updated the screen-time limits") + undeliveredNote(delivery),
      );
      return delivery;
    },
    [signClauseIfConnected, addActivity],
  );

  // Block / un-block an app or game (revoke its launch on the device). A
  // parental rule change, so it is signed when a signer is connected.
  const setBlocked = useCallback(
    async (childId: string, policyId: string, blocked: boolean) => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const existing = child?.policies.find((p) => p.id === policyId);
      if (!existing) return;
      const policy: Policy = { ...existing, blocked };
      const delivery = await signClauseIfConnected(childId, policy);
      if (delivery.outcome === "cancelled") return;
      dispatch({ type: "SET_POLICY", childId, policy });
      const label =
        existing.scope.kind === "app" ? existing.scope.label : "the device";
      addActivity(
        childId,
        "rule-changed",
        (blocked ? `You blocked ${label}` : `You unblocked ${label}`) +
          undeliveredNote(delivery),
      );
    },
    [signClauseIfConnected, addActivity],
  );

  // App-scope save: block + allowed-hours together. Unlike a device policy
  // (whose dimensions each carry their own clause), an app rule is one row in
  // the child's aggregate `appRules` clause — so we hand the signer the FULL
  // current app policy (spread over `existing`), and it rebuilds the whole set.
  // `schedule` undefined lifts any per-app time limit. Signed when connected;
  // otherwise an offline draft (no throw), mirroring the device path.
  const saveAppRule = useCallback(
    async (
      childId: string,
      policyId: string,
      next: { blocked: boolean; schedule?: Schedule },
    ) => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const existing = child?.policies.find((p) => p.id === policyId);
      if (!existing || existing.scope.kind !== "app") return;
      const policy: Policy = {
        ...existing,
        blocked: next.blocked,
        schedule: next.schedule,
      };
      const delivery = await signClauseIfConnected(childId, policy);
      if (delivery.outcome === "cancelled") return;
      dispatch({ type: "SET_POLICY", childId, policy });
      addActivity(
        childId,
        "rule-changed",
        `You updated the rules for ${existing.scope.label}` + undeliveredNote(delivery),
      );
    },
    [signClauseIfConnected, addActivity],
  );

  const addAppLimit = useCallback(
    (childId: string, app: { appId: string; label: string }): Policy => {
      const policy: Policy = {
        id: uid("pol"),
        scope: { kind: "app", appId: app.appId, label: app.label },
      };
      dispatch({ type: "ADD_POLICY", childId, policy });
      addActivity(childId, "rule-changed", `You added controls for ${app.label}`);
      return policy;
    },
    [addActivity],
  );

  const removePolicy = useCallback(
    (childId: string, policyId: string) => {
      const child = stateRef.current.children.find((c) => c.id === childId);
      const policy = child?.policies.find((p) => p.id === policyId);
      const label =
        policy && policy.scope.kind === "app" ? policy.scope.label : "an app";
      dispatch({ type: "REMOVE_POLICY", childId, policyId });
      addActivity(childId, "rule-changed", `You removed controls for ${label}`);
    },
    [addActivity],
  );

  // --- Signed parent decisions -----------------------------------------

  // Assemble the signer's context for a decision on `req`: which child (for
  // the delivery relays), the granted minutes, the schedule tz the grant's
  // end-of-day expiry is computed in, and — for a REAL relay-ingested ask —
  // the wire correlation the GRANT must echo. A simulated request carries no
  // wire block, so its approval never publishes (the demo path stays local).
  const decisionContext = useCallback(
    (req: ChildRequest, minutesGranted?: number): DecisionContext => {
      const child = stateRef.current.children.find((c) => c.id === req.childId);
      const devicePolicy = child?.policies.find((p) => p.scope.kind === "device");
      // The minutes/tz/wire-correlation slice — a pure, separately-tested
      // function (N1 fix, review round 2, 2026-08-03; see its file doc for
      // why this HAD to be pulled out rather than left as an inline ternary).
      const timing = decisionTiming({
        req,
        minutesGranted,
        scheduleTz: devicePolicy?.schedule?.tz,
        budgetTz: devicePolicy?.budget?.tz,
        // Named times: a buckets-only ward carries neither of the above — its
        // buckets clause's own (required) tz is the only one it has (C-1).
        bucketsTz: devicePolicy?.buckets?.tz,
      });
      // install.apk: correlate every real device ask so BOTH answers can travel.
      // The cert is pinned from the CURATED catalog (never the phone's request);
      // an uncurated ask carries no cert, so only a DENY can be published for it
      // (an allow needs the pin, and approveRequest blocks uncurated approvals).
      let install: InstallDecision | undefined;
      if (req.kind === "install.app" && req.reqId && req.nonce && req.machine && req.appId) {
        const app = catalogLookup(req.appId);
        install = {
          reqId: req.reqId,
          nonce: req.nonce,
          machine: req.machine,
          packageName: app?.packageName ?? req.appId,
          signerCertSha256: app?.signerCertSha256,
          source: app?.source ?? "staged",
        };
        // Self-update bootstrap (#44): an ask to install Kintrinsic ITSELF is
        // an UPDATE — without a versionCode pin the device's idempotence
        // guard sees "already installed" and correctly no-ops. Pin the
        // published artifact's versionCode so the device actually commits;
        // with NO manifest loaded, strip the cert so the approval is blocked
        // (uncurated-style) instead of publishing a silently useless grant.
        if (install.packageName === "org.forgesworn.charter") {
          if (updateManifest) {
            install.versionCode = updateManifest.versionCode;
          } else {
            install.signerCertSha256 = undefined;
          }
        }
      }
      return {
        childId: req.childId,
        minutesGranted: timing.minutesGranted,
        tz: timing.tz,
        // The device's OWN exp validity cap consults ONLY these two (never
        // buckets) — passed through separately from `tz` so `realSigner.ts`
        // can clamp correctly even when `tz` resolved from the buckets
        // fallback (review round 2, hardware round 2026-08-03).
        scheduleTz: devicePolicy?.schedule?.tz,
        budgetTz: devicePolicy?.budget?.tz,
        wire: timing.wire,
        install,
        appOpen: timing.appOpen,
      };
    },
    [updateManifest],
  );

  const signDecisionIfConnected = useCallback(
    async (
      requestId: string,
      decision: "approved" | "denied",
      ctx: DecisionContext,
    ): Promise<boolean> => {
      // Offline drafts are for local-only (simulated) asks; a wire-correlated
      // ask throws here instead of faking success with no GRANT published.
      if (
        decisionPath(signer.current!.status().connected, ctx.wire ?? ctx.install ?? ctx.appOpen) ===
        "record-locally"
      ) {
        return true;
      }
      try {
        await signer.current!.signDecision(requestId, decision, ctx);
        return true;
      } catch (err) {
        if (err instanceof SignerCancelled) return false;
        throw err;
      }
    },
    [],
  );

  const approveRequest = useCallback(
    async (id: string, minutesGranted?: number) => {
      const req = stateRef.current.requests.find((r) => r.id === id);
      if (!req) return;
      // A real install ask can only be approved for a CURATED app — that's the
      // only place a trusted signing-cert digest to pin comes from. Refuse
      // rather than fake success (the device would get no grant anyway).
      if (req.kind === "install.app" && req.reqId && req.appId && !catalogLookup(req.appId)) {
        throw new Error(
          `${req.appLabel ?? req.appId} isn't in your trusted app list yet, so it can't be verified. Add it to the catalog first.`,
        );
      }
      const ok = await signDecisionIfConnected(
        id,
        "approved",
        decisionContext(req, minutesGranted),
      );
      if (!ok) return;
      dispatch({ type: "SET_REQUEST_STATUS", id, status: "approved" });
      // install.app / run.program are PER-ARTIFACT and SINGLE-USE: approving
      // grants this one app, just this once — it never creates or mutates a
      // Policy (no standing "can install anything").
      if (req.kind === "install.app" || req.kind === "run.program") {
        const label = req.appLabel ?? "that app";
        const verb = req.kind === "install.app" ? "install" : "run";
        addActivity(
          req.childId,
          "enacted",
          `You approved ${label} to ${verb} — just this once`,
        );
        return;
      }
      const mins = minutesGranted ?? req.minutesRequested;
      const detail = mins ? ` (+${mins} min)` : "";
      // Named times (M-2): a bucket-hit extend credits ONE group — record it
      // structurally so the Today card can add it back onto that group's cap
      // rather than reading the spend inside it as a breach.
      addActivity(
        req.childId,
        "enacted",
        `You approved: ${req.title}${detail}`,
        req.limitHit === "bucket" && req.bucketId && mins
          ? { bucketId: req.bucketId, minutesGranted: mins }
          : undefined,
      );
    },
    [signDecisionIfConnected, decisionContext, addActivity],
  );

  const denyRequest = useCallback(
    async (id: string) => {
      const req = stateRef.current.requests.find((r) => r.id === id);
      if (!req) return;
      // A deny still travels as a signed GRANT (decision "deny", 0 minutes) —
      // the device tells the child "not now" instead of leaving them hanging.
      // For app.open this is the WHOLE answer (no clause change at all — the
      // app is already blocked, and stays that way).
      const ok = await signDecisionIfConnected(id, "denied", decisionContext(req, 0));
      if (!ok) return;
      dispatch({ type: "SET_REQUEST_STATUS", id, status: "denied" });
      // `req.title` is built once at ingestion (`requestCards.ts`) and could
      // in principle carry an already-saved app.open card's raw pkg mid-
      // sentence if it predates that file's own `identityDisplayLabel` fix —
      // belt and braces (F1 review): rebuild the app.open case from the
      // (now-resolved) identity fields directly rather than trust the
      // stored sentence, instead of only every OTHER kind's generic title.
      addActivity(
        req.childId,
        "denied",
        req.kind === "app.open"
          ? `You said not now: Open ${identityDisplayLabel(req.appLabel, req.appId) ?? "this app"}?`
          : `You said not now: ${req.title}`,
      );
    },
    [signDecisionIfConnected, decisionContext, addActivity],
  );

  /**
   * Let a repeat ask go without answering it — see the context type's doc.
   * A plain, synchronous local dispatch: no `signDecisionIfConnected`, no
   * `decisionContext`, no `addActivity` — nothing here can reach the signer
   * or the wire, by construction, unlike `approveRequest`/`denyRequest`
   * above (both of which always go through the signer gate, even when it
   * turns out to resolve locally).
   *
   * Reuses `status` rather than removing the row from `requests`: the
   * request (and its `reqId`, for a real device-originated ask) stays put,
   * so it keeps blocking `ingestDeviceRequest`'s reqId dedupe the same way
   * an "approved"/"denied" request already does — the next relay poll
   * within the 24h window will NOT re-add it. Removing the array entry
   * instead would have un-dismissed it the moment the relay served the same
   * ask again.
   */
  const dismissRequest = useCallback((id: string) => {
    dispatch({ type: "SET_REQUEST_STATUS", id, status: "dismissed" });
  }, []);

  /**
   * Answer an `app.open` ask with one of the one-tap windows: overlay an
   * `AppHold { pkg, allowed, untilUnix }` via the EXISTING hold machinery
   * (`domain/appHolds.ts`) — the same shape a manual hold from Limits sends.
   * The hold OVERLAYS: the standing `blocked`/`askFirst` lists are untouched,
   * so the askFirst-⊆-blocked invariant holds trivially.
   *
   * Composed against the CORRECT baseline (C2, review round 1, 2026-08-03):
   * the asking device's own `apps` OVERRIDE when it splits that control (a
   * split device never reads base — the hold must land where it actually
   * looks), otherwise base. Either way `deviceOverrides` travels through in
   * FULL — omitting it entirely (the original bug) made `pickOverrides`
   * flatten every OTHER device's split the moment this signed `apps` at all
   * (see `savePolicy`'s own doc on this exact failure mode), silently
   * lifting e.g. a laptop's own Steam block.
   *
   * Refuses (never silently invents an empty blocklist — a review-round-1
   * fix) when this session never loaded the child's `apps` policy at all:
   * a fabricated empty blocklist would publish "nothing is blocked" over a
   * ward who may have real rules this guardian just hasn't seen yet.
   *
   * The CLAUSE — the AppHold, the thing that actually opens the app — is
   * signed FIRST, and the plain Decision echo (the courtesy "closes the
   * loop" signal) only AFTER it demonstrably reached the wire (N2, review
   * round 2, 2026-08-03). The reverse order — echo first, clause second —
   * let a declined clause-sign confirm (autoSign off is the DEFAULT; the
   * guardian taps "cancel" on the second of two prompts) slip through
   * `savePolicy`'s own silent `{outcome:"cancelled"}` return: the request
   * still got marked approved and the activity log still said "You let them
   * open Minecraft" while no hold ever landed and the app stayed blocked —
   * the ward's card would have read "You can open it!" over a lie. Nothing
   * is marked approved and no Decision travels unless the clause truly sent.
   */
  const approveAppOpen = useCallback(
    async (id: string, untilUnix: number, window: AppOpenWindow) => {
      const req = stateRef.current.requests.find((r) => r.id === id);
      if (!req || req.kind !== "app.open" || !req.appId) return;
      const child = stateRef.current.children.find((c) => c.id === req.childId);
      const devicePolicy = child?.policies.find((p) => p.scope.kind === "device");
      const currentApps = devicePolicy?.apps;
      if (!currentApps) {
        throw new Error(
          `Can't find ${child?.name ?? "their"} app rules yet — open their Limits once, then try again.`,
        );
      }
      const nowUnix = Math.floor(Date.now() / 1000);
      const minutesGranted = Math.max(0, Math.round((untilUnix - nowUnix) / 60));

      const composed = composeDeviceHold(
        { apps: currentApps, deviceOverrides: devicePolicy.deviceOverrides },
        req.deviceId,
        req.appId,
        "allowed",
        untilUnix,
        nowUnix,
      );
      // The ordering itself is the load-bearing part (N2) — run it through
      // the separately-tested orchestrator rather than reimplementing the
      // sequencing inline here.
      const result = await runApproveAppOpenFlow({
        saveHold: () => savePolicy(req.childId, devicePolicy.id, composed),
        sendDecision: () => signDecisionIfConnected(id, "approved", decisionContext(req, minutesGranted)),
      });
      if (result.outcome === "refused") throw new Error(result.message);
      // The clause landed (the app can open); only the courtesy echo's own
      // confirm was declined. Nothing false to claim, nothing to refuse —
      // just don't mark this ask "answered" when the loop didn't fully close.
      if (result.outcome === "decision-cancelled") return;

      dispatch({ type: "SET_REQUEST_STATUS", id, status: "approved" });
      // Belt and braces (F1 review): resolve through `identityDisplayLabel`
      // rather than trust `req.appLabel`/`req.appId` verbatim.
      addActivity(
        req.childId,
        "enacted",
        `You let them open ${identityDisplayLabel(req.appLabel, req.appId) ?? "that app"} ${APP_OPEN_WINDOW_PHRASE[window]}`,
      );
    },
    [savePolicy, signDecisionIfConnected, decisionContext, addActivity],
  );

  // M-1 (hardware round, 2026-08-03): the guardian's stepped-down grant
  // amount used to live only in Approvals' own `useState`, which resets on
  // remount — exactly what happens when a send fails (C-1's error path:
  // fail → go fix something → come back). Persisting it on the request
  // record itself means the choice survives that round trip.
  const setRequestChosenMinutes = useCallback((id: string, minutes: number) => {
    dispatch({ type: "SET_REQUEST_CHOSEN_MINUTES", id, minutes });
  }, []);

  // --- Signer actions ---------------------------------------------------

  const connectSigner = useCallback(
    async (kind: SignerKind) => {
      await signer.current!.connect(kind);
      syncSigner();
    },
    [syncSigner],
  );

  // Turn on the local-key signer: this phone holds the guardian key and signs
  // clauses directly (no external bunker). Idempotent — safe to call again.
  const enableLocalSigner = useCallback(async () => {
    const next: SignerState = { ...stateRef.current.signer, kind: "local", bunkerUri: undefined };
    try {
      signer.current = buildSigner(next);
      await signer.current.connect("local");
    } catch (e) {
      signer.current = buildSigner({ ...stateRef.current.signer, kind: "none", bunkerUri: undefined });
      throw e;
    }
    dispatch({ type: "SET_SIGNER", signer: signer.current.status() });
  }, [buildSigner]);

  // Recovery: install a restored guardian key (from an encrypted backup) and
  // switch to the local signer over it. The restored pubkey matches every
  // device this guardian already paired, so those pairings revive untouched —
  // no re-pairing. Returns the restored guardian pubkey (hex).
  const restoreGuardianKey = useCallback(
    async (secret: Uint8Array): Promise<string> => {
      importGuardianKey(secret);
      const next: SignerState = {
        ...stateRef.current.signer,
        kind: "local",
        bunkerUri: undefined,
      };
      signer.current = buildSigner(next);
      await signer.current.connect("local");
      dispatch({ type: "SET_SIGNER", signer: signer.current.status() });
      return guardianPubkeyHex();
    },
    [buildSigner],
  );

  // Pair the REAL Signet bunker from a `bunker://…` URI: rebuild the signer as
  // the Signet-backed one, open the NIP-46 session, and persist the URI (so the
  // pairing survives reload). On failure the mock is restored and it rethrows.
  const pairSignet = useCallback(
    async (bunkerUri: string) => {
      const next: SignerState = { ...stateRef.current.signer, kind: "signet", bunkerUri };
      try {
        signer.current = buildSigner(next);
        await signer.current.connect("signet");
      } catch (e) {
        signer.current = buildSigner({ ...stateRef.current.signer, kind: "none", bunkerUri: undefined });
        throw e;
      }
      // status() doesn't carry the store-only bunkerUri — merge it back in.
      dispatch({ type: "SET_SIGNER", signer: { ...signer.current.status(), bunkerUri } });
    },
    [buildSigner],
  );

  const disconnectSigner = useCallback(async () => {
    await signer.current!.disconnect();
    // Drop the pairing + fall back to the mock (clears bunkerUri).
    const cleared: SignerState = { connected: false, kind: "none", autoSign: stateRef.current.signer.autoSign };
    signer.current = buildSigner(cleared);
    dispatch({ type: "SET_SIGNER", signer: cleared });
  }, [buildSigner]);

  const setAutoSign = useCallback(
    async (on: boolean) => {
      await signer.current!.setAutoSign(on);
      syncSigner();
    },
    [syncSigner],
  );

  const value = useMemo<CharterContextValue>(
    () => ({
      state,
      addChild,
      editChild,
      removeChild,
      addDevice,
      beginPairing,
      confirmPairing,
      claimDevice,
      unclaimed,
      reconnectDevice,
      deviceStatus,
      usageHistory,
      updateManifest,
      debManifest,
      refreshNow,
      refreshing,
      refreshNotice,
      giveTime,
      standDown,
      openMaintenanceWindow,
      sendCharterUpdate,
      phonePairing,
      beginPhonePairing,
      cancelPhonePairing,
      enrollDevice,
      unpairDevice,
      sendPairOffer,
      removeDevice,
      simulateDeviceRequest,
      setSchedule,
      setBudget,
      savePolicy,
      setBlocked,
      saveAppRule,
      addAppLimit,
      removePolicy,
      approveRequest,
      denyRequest,
      approveAppOpen,
      dismissRequest,
      setRequestChosenMinutes,
      connectSigner,
      enableLocalSigner,
      restoreGuardianKey,
      pairSignet,
      disconnectSigner,
      setAutoSign,
      pendingSignature,
      resolveSignature,
    }),
    [
      state,
      addChild,
      editChild,
      removeChild,
      addDevice,
      beginPairing,
      confirmPairing,
      claimDevice,
      unclaimed,
      reconnectDevice,
      deviceStatus,
      usageHistory,
      updateManifest,
      debManifest,
      refreshNow,
      refreshing,
      refreshNotice,
      giveTime,
      standDown,
      openMaintenanceWindow,
      sendCharterUpdate,
      phonePairing,
      beginPhonePairing,
      cancelPhonePairing,
      enrollDevice,
      unpairDevice,
      sendPairOffer,
      removeDevice,
      simulateDeviceRequest,
      setSchedule,
      setBudget,
      savePolicy,
      setBlocked,
      saveAppRule,
      addAppLimit,
      removePolicy,
      approveRequest,
      denyRequest,
      approveAppOpen,
      dismissRequest,
      setRequestChosenMinutes,
      connectSigner,
      enableLocalSigner,
      restoreGuardianKey,
      pairSignet,
      disconnectSigner,
      setAutoSign,
      pendingSignature,
      resolveSignature,
    ],
  );

  // DEV-ONLY test seam: there is no relay in a local dev/Playwright-smoke
  // session to actually deliver a STATUS heartbeat, so honest-attribution's
  // guardian-only fields (unrecognisedTodaySecs, AppRef.userInstalled) have
  // no other way to appear in the running app for a screenshot. Stripped
  // from a production build (`import.meta.env.DEV` is `false` there, and
  // dead-code-eliminated) — never reachable outside a local dev server.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    (window as unknown as { __charterDevStatus?: typeof ingestDeviceStatus }).__charterDevStatus =
      ingestDeviceStatus;
  }, [ingestDeviceStatus]);

  return (
    <CharterContext.Provider value={value}>{children}</CharterContext.Provider>
  );
}

export function useCharter(): CharterContextValue {
  const ctx = useContext(CharterContext);
  if (!ctx) {
    throw new Error("useCharter must be used within a CharterProvider");
  }
  return ctx;
}

/** Convenience: pending requests count for the tab badge. */
export function pendingCount(state: CharterState): number {
  return state.requests.filter((r) => r.status === "pending").length;
}

/** Convenience re-export so screens import the selector from one place. */
export function childStatus(child: Child, now = Date.now()): StatusForResult {
  return statusFor(child, now);
}
