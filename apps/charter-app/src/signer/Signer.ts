import type { Policy, SignerKind, SignerState } from "../domain/types";
import type { TimeExtendLimitHit, UpdateManifest } from "../wire/types";

/**
 * Wire correlation for a REAL device-originated request — everything the GRANT
 * must echo (`reqId`/`nonce`/`limitHit`) plus who asked and which machine the
 * signed grant must be gift-wrapped back to. A simulated/demo request has no
 * wire block, so its decision never publishes anything.
 */
export interface DecisionWire {
  /** 32-byte hex, echoed verbatim in the GRANT. */
  reqId: string;
  /** 32-byte hex, echoed verbatim in the GRANT. */
  nonce: string;
  /** The asking device's pubkey (hex) — the GRANT's gift-wrap recipient. */
  machine: string;
  /** The managed user's pubkey (hex) — who asked. */
  subject: string;
  /** Which limit was hit — echoed so the device's params-echo check is exact. */
  limitHit: TimeExtendLimitHit;
  /** Present only when `limitHit === 'bucket'` — the specific named-times
   *  group the REQUEST named. Echoed verbatim into the GRANT, exactly like
   *  `limitHit` itself, on both allow and deny. */
  bucketId?: string;
  /** The named time the REQUEST asked for more of (contract §Times). Echoed
   *  verbatim into the GRANT on allow and deny; absent = Free time. */
  time?: string;
}

/**
 * Wire correlation for a real `app.open` ask — everything the plain Decision
 * echo (`{pkg, minutesGranted}`, see `wire/grant.ts`'s `buildAppOpenGrant`)
 * needs. It carries no ENACTABLE effect of its own — the actual permission
 * travels as a SEPARATE `apps`-clause save carrying an `AppHold`, which the
 * store builds alongside this from `ctx.childId`. `minutesGranted` rides on
 * the shared `DecisionContext.minutesGranted`, same slot `time.extend` uses.
 */
export interface AppOpenDecisionWire {
  /** 32-byte hex, echoed verbatim in the GRANT. */
  reqId: string;
  /** 32-byte hex, echoed verbatim in the GRANT. */
  nonce: string;
  /** The asking device's pubkey (hex) — the GRANT's gift-wrap recipient. */
  machine: string;
  /** The managed user's pubkey (hex) — who asked. */
  subject: string;
  /** Echoed verbatim from the REQUEST — the params-echo check, same as
   *  every other op. */
  pkg: string;
}

/**
 * Wire correlation + the TRUSTED pin for a real `install.apk` ask. `reqId`/
 * `nonce` echo the pending REQUEST; `signerCertSha256` comes from the curated
 * catalog (never the phone's request) and is what the device enforces signing
 * continuity against. Absent on a time.extend or simulated decision.
 */
export interface InstallDecision {
  /** 32-byte hex, echoed verbatim in the GRANT. */
  reqId: string;
  /** 32-byte hex, echoed verbatim in the GRANT. */
  nonce: string;
  /** The asking device's pubkey (hex) — the GRANT's gift-wrap recipient. */
  machine: string;
  packageName: string;
  /** Lowercase-hex SHA-256 of the vetted signing cert (the provenance pin).
   *  Required to APPROVE (that's the security anchor); absent is only valid for
   *  a DENY, which the device never checks a cert against. */
  signerCertSha256?: string;
  /** Optional minimum version the device will accept. */
  versionCode?: number;
  source: "staged";
}

/** Everything a signer needs to turn an approve/deny into a delivered GRANT. */
export interface DecisionContext {
  /** The child the request belongs to (resolves the delivery relays). */
  childId: string;
  /** `time.extend`: minutes the parent granted (wire-clamped to 0..=1440). */
  minutesGranted?: number;
  /** IANA tz the grant's NATURAL end-of-day is computed in (`pickExtendTz`'s
   *  resolved dimension — schedule, else budget, else, only when neither
   *  clause exists, the buckets clause's own tz). Absent = the phone's local
   *  midnight. */
  tz?: string;
  /** The child's ACTUAL schedule/budget clause tz's, separate from `tz` —
   *  `time.extend`'s grant-building clamps `exp` to the device's own
   *  validity cap (`wire/grant.ts`'s `deviceExtendCapEod`, which mirrors
   *  `charter-spine::time_extend_eod` and consults ONLY these two, never
   *  buckets). Needed even when `tz` above resolved from neither of them
   *  (a buckets-only ward), so that case still clamps correctly instead of
   *  signing a grant the device silently drops (review round 2, hardware
   *  round 2026-08-03). */
  scheduleTz?: string;
  budgetTz?: string;
  /** `time.extend` wire correlation; absent = not a time.extend ask. */
  wire?: DecisionWire;
  /** `install.apk` wire correlation + the catalog-pinned cert; absent = not an
   *  install ask (or the app isn't curated, so it must not be published). */
  install?: InstallDecision;
  /** `app.open` wire correlation; absent = not an app.open ask. */
  appOpen?: AppOpenDecisionWire;
}

/**
 * Where a gift goes and what it buys, beyond the minutes (contract §Times).
 * Both fields default to today's behaviour, so a caller that passes neither is
 * byte-identical to before: Free time, every paired device.
 */
export interface GiftOptions {
  /** The named-time id the minutes buy. Absent (or `"free"`) = Free time. */
  time?: string;
  /** Used only to word the confirm sheet ("30 minutes of Learning time"). */
  timeLabel?: string;
  /** `Device.id`s to wrap the gift to. Absent = every paired device; an empty
   *  list is refused (no recipient is a mistake, not "everyone"). */
  deviceIds?: string[];
}

/**
 * Signer — the parent's authorization helper.
 *
 * IMPORTANT (design intent): the signer signs the PARENT'S decision. It never
 * decides FOR the child and it never auto-approves a child's request. It is a
 * pen, not a judge: the parent (or their policy) makes the call; the signer
 * just affixes the seal.
 *
 * Two flavors of authorization:
 *   - signClause:   the parent changes a rule (a schedule/budget for a child).
 *   - signDecision: the parent approves/denies a specific child request.
 *
 * autoSign:
 *   - false → the UI MUST surface an explicit confirm step ("Approve in
 *     Signet") and the returned promise only resolves once the parent confirms.
 *   - true  → the signer signs immediately, no extra prompt.
 *
 * This interface is deliberately transport-agnostic. The demo `mockSigner`
 * fulfills it locally; a real NIP-46 / Signet / Heartwood remote signer plugs
 * in behind this exact same surface later with no screen changes.
 */
/**
 * What a clause send resolves with. `wraps` are the ids of the wraps the
 * Android carrier took for home-network delivery, so the activity line can
 * follow their ACKs ("applied on …" or a warning); absent when none went that
 * way (relays only, or a signer with no home-network route).
 */
export interface SignOutcome {
  ok: true;
  wraps?: string[];
}

/** A `SignOutcome` that also reports the `issuedAt` (unix seconds) the clause was signed with. */
export interface SignOutcomeAt extends SignOutcome {
  issuedAt: number;
}

export interface Signer {
  /** Offline recovery: signed JSON, transferred locally to the ward. */
  signRelayConfig?(machine: string, relays: string[], label: string): Promise<string>;
  /** Current connection + auto-sign state (synchronous snapshot). */
  status(): SignerState;

  /** Pair a signer of the given kind. Resolves with the new state. */
  connect(kind: SignerKind): Promise<SignerState>;

  /** Forget the signer. */
  disconnect(): Promise<void>;

  /** Turn the "sign without asking" convenience on/off. */
  setAutoSign(on: boolean): Promise<void>;

  /** Sign a rule change (a new/updated policy) for a child. */
  signClause(childId: string, policy: Policy): Promise<SignOutcome>;

  /**
   * Publish each device its consolidated cross-device usage view
   * (USAGE_SYNC 31115 — B3 pooled budget). Optional: only signers with a real
   * publish path implement it; the store no-ops when absent.
   */
  publishUsageSyncs?(
    childId: string,
    payloadsByDevice: Record<string, import("../wire/usageSync").UsageSyncPayload>,
  ): Promise<{ ok: true }>;

  /**
   * Sign + publish the `update` clause (#44): "run Kintrinsic at the manifest's
   * versionCode or newer, fetched from `url`". Delivered to every governed
   * device the child uses — a device already at-or-past the version ignores
   * it (level-triggered convergence).
   */
  /** Open a short, signed maintenance window (see RealSigner). */
  signMaintenanceClause?(childId: string, minutes: number): Promise<SignOutcome>;
  /** Give minutes with no ask outstanding (and after a "no"). Today only.
   *  `groupId` tops up that named-times group's own pool instead of the
   *  whole-device one; absent = today's plain behaviour, unchanged. */
  signGiftClause?(
    childId: string,
    minutes: number,
    tz?: string,
    groupId?: string,
    opts?: GiftOptions,
  ): Promise<SignOutcome>;
  /**
   * Answer a `time.extend` ask with a DIFFERENT type of time than it asked
   * for (contract §Times): a `minutesGranted: 0` grant that closes the ask,
   * plus a gift carrying `timeId` (absent = Free time), both wrapped to the
   * asking device only. `ctx.wire` is required — there is nothing to answer
   * without a real ask. One confirm covers both.
   */
  answerAskWithOtherTime?(
    requestId: string,
    ctx: DecisionContext,
    minutes: number,
    timeId?: string,
    /** Wording for the confirm sheet only; the raw id is shown if absent. */
    timeLabel?: string,
  ): Promise<SignOutcome>;
  /**
   * Call a stand-down: "finish up now", then locked for the rest of the ward's
   * day. Pass `lift: true` to allow them back on — that publishes a stand-down
   * already expired, which supersedes the standing one.
   */
  signStandDownClause?(
    childId: string,
    opts?: { lift?: boolean; graceSecs?: number; tz?: string },
  ): Promise<SignOutcome>;
  signUpdateClause(
    childId: string,
    manifest: UpdateManifest,
    url: string,
  ): Promise<SignOutcome>;
  /**
   * Sign + publish the Linux `update` clause (charterd ≥ 0.7.12) to ONE
   * laptop: gift-wrapped only to `devicePubkey`, still carrying the child's
   * `subject`, so the ward's phones keep their own `update` clause. Throws when
   * `devicePubkey` is not one of the child's paired devices.
   */
  signDebUpdateClause?(
    childId: string,
    devicePubkey: string,
    deb: { versionCode: number; versionName: string; sha256: string },
    url: string,
  ): Promise<SignOutcome>;

  /**
   * Sign + publish the `debugging` clause (USB / wireless debugging) to ONE
   * device: gift-wrapped only to `devicePubkey`, still carrying the child's
   * `subject`. `until` (unix seconds) is dropped when `allow` is false. Throws
   * when `devicePubkey` is not one of the child's paired devices. Resolves with
   * the `issuedAt` (unix seconds) the clause carries; a send held for the home
   * network throws `QueuedForHomeNetworkError` with that same value on its
   * `issuedAt`.
   */
  signDebuggingClause(
    childId: string,
    devicePubkey: string,
    grant: { allow: boolean; until?: number },
  ): Promise<SignOutcomeAt>;

  /**
   * Sign + publish a parent-gated RELEASE to a device's machine pubkey — the
   * unpair. The device drops its pairing and all enforcement. A no-op for a
   * device with no real key (nothing to release).
   *
   * `label` is the device's human name, used ONLY in the confirm sheet (S9):
   * this is the most destructive wire action Kintrinsic has, and a parent asked
   * to confirm "Release device ab12cd34…" has been told nothing they can act
   * on. Optional so a caller without a name still works — the sheet falls
   * back to the key prefix.
   */
  releaseDevice(
    machinePubkey: string,
    relays: string[],
    label?: string,
  ): Promise<{ ok: true }>;

  /**
   * Scan-to-pair: offer to be pinned by the ward whose QR we just scanned,
   * echoing the one-time `token` from its screen as proof of physical
   * presence. The ward pins whoever sealed this, so no key is claimed here.
   */
  sendPairOffer(machinePubkey: string, token: string, relays: string[]): Promise<{ ok: true }>;

  /**
   * Sign the parent's decision on a child's request. When `ctx.wire` is
   * present (a real device-brokered ask) the signer also builds, signs, and
   * gift-wraps the GRANT to the asking machine; without it the decision is
   * recorded locally only.
   */
  signDecision(
    requestId: string,
    decision: "approved" | "denied",
    ctx?: DecisionContext,
  ): Promise<{ ok: true }>;
}
