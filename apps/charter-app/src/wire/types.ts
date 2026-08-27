// The Kintrinsic device-broker WIRE types — the exact shapes `charterd` verifies.
// These mirror `spec/contract.md` (GrantSchedule / GrantBudget / ClausePayload)
// and the Rust `charter-proto` / `charter-schedule` structs. Field names are
// load-bearing (camelCase on the wire); `v` + `issuedAt` are required.
//
// IMPORTANT — `paused` is INVERTED vs the app's domain model:
//   app  Schedule.paused = "lifts the schedule (always allowed)"
//   wire GrantSchedule.paused = "blocks ALL time"
// The mapping in ./clause.ts translates this; never copy `paused` across.

import type { ScheduleWindow } from "../domain/types";

export type { ScheduleWindow };

export interface WeeklySchedule {
  mon?: ScheduleWindow[];
  tue?: ScheduleWindow[];
  wed?: ScheduleWindow[];
  thu?: ScheduleWindow[];
  fri?: ScheduleWindow[];
  sat?: ScheduleWindow[];
  sun?: ScheduleWindow[];
}

/** Contract `GrantSchedule` (when the child may use the device). */
export interface GrantSchedule {
  v: 1;
  tz: string;
  /** Wire semantics: true = BLOCK everything. */
  paused?: boolean;
  weekly: WeeklySchedule;
  overrides?: Record<string, ScheduleWindow[]>;
  issuedAt: number;
}

/** Contract `GrantBudget` (how much time per day/week). */
export interface GrantBudget {
  v: 1;
  tz: string;
  dailyMinutes?: number | null;
  weeklyMinutes?: number | null;
  weekStart?: "sun" | "mon";
  /** Wire semantics: true = quota 0 (block all time). */
  paused?: boolean;
  /** Soft tombstone — the budget no longer constrains. */
  revoked?: boolean;
  /**
   * WHAT the allowance is spent on (2026-08-06, mirrors the Rust
   * `charter_schedule::TimeModel`). Absent means `"session"` — every clause
   * signed before this field existed keeps its exact meaning, so no ward
   * changes behaviour on upgrade. `./clause.ts`'s `budgetToGrant` therefore
   * emits this ONLY for `"named"`, never `"session"` explicitly.
   */
  model?: "session" | "named";
  issuedAt: number;
}

export type ClauseKind =
  | "schedule"
  | "budget"
  | "content"
  | "apps"
  | "learning"
  | "apprules"
  | "tethering"
  | "update"
  | "lifeline"
  | "buckets"
  | "maintenance"
  | "gift"
  | "standdown"
  | "listening"
  | "alwaysavailable";

/**
 * Contract `GrantListening` (spec 2026-07-29) — what happens to audio ALREADY
 * PLAYING when the lock lands. Mirrors `charter_proto::ListeningBody`.
 *
 * SEMANTICS: unlike most clauses here, an absent or unusable one means `stop` —
 * this clause loosens enforcement (it keeps an app alive past a lock), so the
 * device fails CLOSED on it. The exemption also requires the named app to be
 * genuinely producing audio, which the DEVICE decides; the guardian's list says
 * which apps may, not when.
 */
export interface GrantListening {
  v: 1;
  issuedAt: number;
  mode: "stop" | "continue" | "grace";
  /** `grace` only, 1..240. */
  graceMinutes?: number;
  apps: string[];
}

/**
 * Contract `AlwaysAvailableApp` — one app open at any hour.
 *
 * `untilUnix` is ABSOLUTE, never a duration, for the same reason `AppHold`'s
 * is: a duration restarts every time the stored clause is re-read and the
 * grant would never end. Absent ⇒ standing, no end.
 */
export interface AlwaysAvailableEntry {
  pkg: string;
  untilUnix?: number;
}

/**
 * Contract `GrantAlwaysAvailable` (spec 2026-08-03) — apps openable at any
 * hour. Mirrors `charter_proto::AlwaysAvailableBody`.
 *
 * SEMANTICS: like `listening` and unlike most clauses here, an absent or
 * unusable one means NOTHING is exempt. It loosens enforcement, so it fails
 * closed.
 *
 * Exempt from the schedule and budget locks ONLY — never a stand-down, never a
 * malformed charter, never a standing block in `apps`.
 */
export interface GrantAlwaysAvailable {
  v: 1;
  issuedAt: number;
  apps: AlwaysAvailableEntry[];
}

/**
 * Contract `LifelineBody` (spec D9) — 1–3 guardian numbers the ward's lock
 * screen can always dial (mirrors the Rust `charter_proto::LifelineBody`,
 * camelCase). The device validates fail-closed: a malformed body renders NO
 * call buttons, so the wire side must send only clean entries.
 */
export interface GrantLifeline {
  v: 1;
  numbers: { label: string; number: string }[];
  issuedAt: number;
  /** Optional (v2): the device resolves the region-correct emergency number
   *  itself — no digit string ever crosses the wire. */
  emergencyServices?: boolean;
  /** Optional: offer a torch on the shade. Older wards ignore the field
   *  (the body doesn't deny unknown keys), so this needs no ship-order guard. */
  torch?: boolean;
  /** Optional (v2): the ward's emergency override. */
  breakGlass?: { enabled: boolean; scope: "calls" | "full"; durationMinutes: number };
}

// --- update clause (guardian-directed Kintrinsic self-update, #44) -------------

/**
 * Contract `UpdateAppBody` — "this device runs `packageName` at `versionCode`
 * or newer, fetched from `url`" (mirrors the Rust `charter_proto::UpdateAppBody`,
 * camelCase). Replace-the-state; a device at-or-past the version does nothing.
 * Both digests must hold on the device before commit.
 */
export interface GrantUpdate {
  v: 1;
  packageName: string;
  versionCode: number;
  versionName: string;
  url: string;
  apkSha256: string;
  signerCertSha256: string;
}

/** The published artifact manifest the site serves at /charter-apk.json. */
export interface UpdateManifest {
  /** Absolute https URL of the artifact on Blossom (content-addressed). The
   *  D3 artifact-hosting decoupling: the artifact is no longer served from
   *  this origin, so the origin-JSON fallback names its Blossom URL directly.
   *  Preferred over `path` by every consumer. */
  url?: string;
  /** Legacy same-origin path (pre-D3 manifests). Fallback only; `url` wins. */
  path?: string;
  versionName: string;
  versionCode: number;
  apkSha256: string;
  certSha256: string;
  sizeBytes: number;
  builtAt: string;
}

// --- appRules clause (per-app block + allowed-hours) ------------------------

/**
 * Contract `AppRule` — one specific app/game governed by block and/or its own
 * allowed-hours. Mirrors the Rust `charter_schedule::AppRule` (camelCase). `pkg`
 * is the on-device identity (Android package / Linux exec or flatpak id).
 */
export interface AppRule {
  pkg: string;
  label?: string;
  /** Blocked outright — may never open, regardless of schedule. */
  blocked: boolean;
  /** Optional per-app allowed-hours (usable only inside these windows). */
  schedule?: GrantSchedule;
}

/**
 * Contract `GrantAppRules` — the guardian's FULL per-app rule set for one child
 * (replace-the-set, like `learning`). One clause carries every rule.
 */
export interface GrantAppRules {
  v: 1;
  rules: AppRule[];
  issuedAt: number;
}

// --- buckets clause (named app allowances) ----------------------------------

/**
 * Contract `AppBucket` — a named set of apps with its own daily allowance
 * ("Play", 60 min). Members use the SAME identity vocabulary as `AppRule.pkg`
 * (Android package id, or Linux exec path / flatpak id), so a bucket listing a
 * laptop's games never matches anything on a phone.
 */
export interface AppBucket {
  /** Stable [a-z0-9-] slug — keys the device's day counter. */
  id: string;
  label: string;
  apps: string[];
  /** The bucket's daily allowance in minutes (1..=1440). At least one of
   *  `dailyMinutes`/`weeklyMinutes` is required; both may be set. */
  dailyMinutes?: number;
  /** The bucket's weekly allowance in minutes (1..=10080). */
  weeklyMinutes?: number;
}

/**
 * Contract `GrantBuckets` — every bucket for one child (replace-the-set).
 * Generalises the learning bucket: learning time is FREE, a bucket here is
 * CAPPED. Spending one closes THAT BUCKET, never the device.
 *
 * **Versioning (compat).** `v: 2` adds the weekly axis (`weeklyMinutes`);
 * `dailyMinutes` went from required to optional. **Producer rule:**
 * Kintrinsic emits `v: 1` iff EVERY bucket in the set has `dailyMinutes` set
 * (so an unchanged all-daily set stays byte-identical to what a pre-weekly
 * ward already understands), and `v: 2` the moment ANY bucket is
 * weekly-only. A pre-weekly ward does not recognise `v: 2` and — per the
 * buckets fail-open doctrine — caps NOTHING until it updates.
 */
export interface GrantBuckets {
  v: 1 | 2;
  buckets: AppBucket[];
  paused?: boolean;
  /** Week-start day for the weekly axis. Absent = 'mon' — mirrors
   *  `GrantBudget.weekStart`. */
  weekStart?: "sun" | "mon";
  /** IANA tz the day/week boundary is computed in. */
  tz: string;
  issuedAt: number;
}

/**
 * Contract `MaintenanceBody` — a guardian-opened window during which the ward
 * stands down its install lock, so a cabled phone can be repaired without
 * removing Device Owner. An ABSOLUTE expiry, never a duration: a duration
 * would restart every time the stored clause was re-read and the window would
 * never shut.
 */
export interface GrantMaintenance {
  v: 1;
  untilUnix: number;
  issuedAt: number;
}

// --- tethering clause (hotspot posture) -------------------------------------

/**
 * What the guardian allows for the ward's hotspot. `none` = blocked; `raw` =
 * the system hotspot (tethered devices get UNFILTERED internet — the UI must
 * say so); `filtered` = Kintrinsic's own hotspot, where guest devices ride the
 * on-phone filter (the ward's own device stays protected either way).
 */
export type TetherAllow = "none" | "raw" | "filtered";

/**
 * Contract `GrantTethering` — the ward's standing hotspot posture (mirrors the
 * Rust `charter_schedule::GrantTethering`, camelCase). Replace-the-state. A
 * time-boxed grant sets `until` (unix seconds); the allowance ends AT `until`.
 * Absent `until` = until a superseding clause revokes it.
 */
export interface GrantTethering {
  v: 1;
  issuedAt: number;
  allow: TetherAllow;
  until?: number;
}

// --- apps clause (standing per-app control, D3) -----------------------------

export type AppPosture = "blocklist" | "allowlist";

/** What a held app IS until its hold ends. */
export type HoldState = "allowed" | "blocked";

/**
 * Contract `AppHold` — a time-boxed departure from the standing lists, for ONE
 * app (mirrors the Rust `charter_proto::AppHold`, camelCase).
 *
 * `untilUnix` is ABSOLUTE, never a duration: a duration restarts every time the
 * stored clause is re-read and the hold would never end. The device resolves it
 * against its own clock at every tick, so a reboot inside a hold does not extend
 * it by a second.
 */
export interface AppHold {
  pkg: string;
  state: HoldState;
  /** Unix seconds. The hold ends AT this instant. */
  untilUnix: number;
}

/**
 * Contract `GrantApps` — the device-enforced standing per-app policy (mirrors
 * the Rust `charter_proto::GrantApps`, camelCase). Package names identify apps.
 * `blocklist` blocks `blocked`; `allowlist` runs ONLY `allowed`. `paused` lifts
 * the policy (the app's "off").
 */
export interface GrantApps {
  v: 1;
  posture: AppPosture;
  blocked?: string[];
  allowed?: string[];
  paused?: boolean;
  /**
   * Time-boxed overrides of the lists above; omitted when empty. The device
   * folds any live hold into the lists before enforcing, so an older warden that
   * does not know this field keeps enforcing the standing lists rather than
   * rejecting the clause — which is why this ships at `v: 1`. Kintrinsic
   * version-gates the feature instead (`wardenSupport`, `appHold`).
   */
  holds?: AppHold[];
  /**
   * Presentation hint: apps the guardian surface should offer an "ask to
   * open" affordance for, instead of a flat "Blocked" (named times' "On
   * request" policy). Every listed pkg MUST also appear in `blocked` —
   * enforcement reads `blocked` only and never consults `askFirst`, so an
   * on-request app is genuinely closed until asked-for and granted on every
   * warden, old or new. An old warden that has never heard of `askFirst`
   * ignores the field and keeps enforcing `blocked` exactly as before:
   * blocked, no ask button, fail CLOSED — never fail open.
   */
  askFirst?: string[];
  /**
   * Packages the device should HIDE outright — gone from the launcher, the
   * app drawer and Settings, as if never installed (Android Device Owner's
   * `setApplicationHidden`). Omitted when empty.
   *
   * Deliberately its OWN axis, not a posture or a list the block/allow rules
   * touch: a Samsung tablet ships with a screenful of OEM bloat, and since
   * ward 0.6.9 locks USB debugging by design there is no cable left to
   * uninstall it with — the guardian's only route is this clause. It is
   * therefore NOT lifted by `paused` either: pausing app CONTROL ("no app is
   * blocked today") must not silently restore forty preinstalled apps to a
   * child's home screen. Reversible by dropping the package from the list.
   *
   * Ships at `v: 1` for the same reason `holds` did — an older warden that
   * has never heard of the field ignores it and keeps enforcing the rest of
   * the clause rather than rejecting it. Kintrinsic version-gates the
   * affordance instead (`wardenSupport`, `appHide`).
   */
  hidden?: string[];
  issuedAt: number;
}

// --- learning clause (time-free learning apps) ------------------------------

export type LearningAppKind = "site" | "native";

/**
 * Contract `LearningApp` (mirrors Rust `charter_proto::LearningApp`). Site
 * apps carry a VERIFIED domain closure + launch URL (materialised on-device
 * as a resolver-pinned app window); native apps carry an exec identity.
 */
export interface LearningApp {
  id: string;
  label: string;
  kind: LearningAppKind;
  domains?: string[];
  url?: string;
  exec?: string;
  /** Ward-writable path the guardian vouches for (advisory identity). */
  trusted?: boolean;
  /**
   * Whether time in this app is FREE. Absent means free — every clause
   * signed before this field existed carries exactly that meaning, so no
   * ward changes behaviour on upgrade. `false` splits the two jobs this
   * clause does (defining a pinned site-app window vs granting free time
   * for it): the window still materialises, but the seconds are charged
   * like any other app — see `domain/types.ts`'s `LearningAppSel` doc.
   */
  free?: boolean;
}

/**
 * Contract `GrantLearning` — the guardian's time-free learning apps. Focused
 * time in one credits the learning bucket instead of draining screen time.
 * `capMinutes` absent = uncapped; `paused` lifts the policy.
 */
export interface GrantLearning {
  v: 1;
  apps: LearningApp[];
  capMinutes?: number;
  paused?: boolean;
  issuedAt: number;
}

// --- content clause (web-content control) ----------------------------------

export type ContentPosture = "allowlist" | "blocklist";
export type ContentAgeTier = "young" | "older";
export type YoutubeRestrict = "off" | "moderate" | "strict";

/**
 * Contract `GrantContent` — the device-enforced web-content clause (mirrors the
 * Rust `charter_content::GrantContent`, camelCase). v1 authors the parent-
 * authoritative fields; curators/categories are wired but left empty.
 * SEMANTICS (do not confuse): `revoked` = filtering LIFTED (unrestricted);
 * `paused` = ALL web blocked (fail-closed). The app's "off" is `revoked`.
 */
export interface GrantContent {
  v: 1;
  tz?: string;
  posture: ContentPosture;
  ageTier: ContentAgeTier;
  curators?: string[];
  quorumN?: number;
  blockCategories?: string[];
  safeSearch?: boolean;
  youtubeRestrict?: YoutubeRestrict;
  parentAllow?: string[];
  parentDeny?: string[];
  paused?: boolean;
  revoked?: boolean;
  issuedAt: number;
}

/**
 * Contract `GiftBody` — minutes the guardian gave without being asked.
 *
 * Additive, not replace-the-state: `id` is the device extension ledger's
 * idempotency key, so a clause the device re-reads every tick applies exactly
 * once, and a SECOND gift (a new id) genuinely adds again.
 */
/**
 * Contract `StandDownBody` — the guardian said "finish up now".
 *
 * Replace-the-state, and the mirror of a gift: where a gift ADDS minutes, this
 * CAPS them at `graceSecs` and then holds them at zero. Lifting it is a NEWER
 * stand-down whose `expiresAt` has already passed, which the device's monotonic
 * issuedAt floor orders for us.
 *
 * The lock instant is deliberately absent: the DEVICE starts the grace from when
 * it first saw this `id`, so relay transit (or an hour spent offline) cannot eat
 * into the warning the ward is owed.
 */
export interface GrantStandDown {
  v: 1;
  issuedAt: number;
  /** Unique per stand-down — the device pins first-sight against this. */
  id: string;
  /** Unix seconds; end of day in the WARD's tz. The midnight lapse. */
  expiresAt: number;
  /** Seconds of warning before the lock lands. Device clamps to 1..=600. */
  graceSecs: number;
}

export interface GrantGift {
  v: 1;
  issuedAt: number;
  /** Unique per gift — the ledger applies each id once. */
  id: string;
  minutes: number;
  /** Unix seconds; end of day in the CHILD's tz. Dead after this. */
  expiresAt: number;
  /**
   * The named-times bucket this gift tops up (its own per-group extension
   * pool). Absent = the whole-device schedule/budget pool, exactly as before
   * named times existed. Stays `v: 1` — additive, and the body does not deny
   * unknown keys — so an OLD ward simply ignores `groupId` and lands the gift
   * as device time anyway. That is deliberate: it errs generous (never "Mum
   * gave me time and nothing happened"), and the gap closes on its own the
   * moment the ward updates.
   */
  groupId?: string;
}

/** Contract `ClausePayload` — the inner content of a CHARTER_DEVICE_CLAUSE (31113). */
export interface ClausePayload {
  v: 1;
  kind: ClauseKind;
  /** The child this clause targets (== Signet dependantId hex). Absent = single-child. */
  subject?: string;
  issuedAt: number;
  body:
    | GrantSchedule
    | GrantBudget
    | GrantContent
    | GrantApps
    | GrantLearning
    | GrantAppRules
    | GrantTethering
    | GrantUpdate
    | GrantLifeline
    | GrantBuckets
    | GrantMaintenance
    | GrantGift
    | GrantStandDown
    | GrantListening
    | GrantAlwaysAvailable;
}

// ---------------------------------------------------------------------------
// Device brokering — REQUEST (31111) / GRANT (31112), `time.extend` (v1 frozen)
// ---------------------------------------------------------------------------

/** The op vocabulary of the device-broker surface (contract `RequestPayload.op`). */
export type RequestOp =
  | "install.flatpak"
  | "install.apk"
  | "exec.allow"
  | "time.extend"
  | "app.open";

/** Where an approved APK comes from (fail-closed; v1 = parent-staged). */
export type ApkSource = "staged";

/** `install.apk` GRANT params — `signerCertSha256` pins provenance and the
 *  device enforces signing continuity BEFORE the installer session commits. */
export interface InstallApkGrantParams {
  packageName: string;
  versionCode?: number;
  signerCertSha256: string;
  source: ApkSource;
}

/** Which limit the child hit — echoed VERBATIM from REQUEST to GRANT so the
 *  device's params-echo check is exact. `bucket` = a named-times group hit
 *  its own allowance instead of the whole-device wall (see `bucketId`). */
export type TimeExtendLimitHit = "schedule" | "budget" | "bucket";

/** Contract `TimeExtendRequestParams` — what the child asked for. The `reason`
 *  is child-authored and PRIVATE: it rides only the E2E gift-wrap, never
 *  audit tags or logs. */
export interface TimeExtendRequestParams {
  /** u16, 1..=1440. */
  minutesRequested: number;
  /** ≤280 chars, private. */
  reason?: string;
  limitHit: TimeExtendLimitHit;
  /** Present only when `limitHit === 'bucket'` — the specific bucket asked
   *  about. Echoed VERBATIM into the grant, exactly like `limitHit` itself;
   *  absent bytes from a pre-buckets device or guardian still parse as unset. */
  bucketId?: string;
}

/** Contract `TimeExtendGrantParams` — what the guardian granted. May be LESS
 *  than requested; `0` is a no-op on the device. */
export interface TimeExtendGrantParams {
  /** u16, 0..=1440. */
  minutesGranted: number;
  limitHit: TimeExtendLimitHit;
  /** Echoed verbatim from the REQUEST when `limitHit === 'bucket'`. A granted
   *  `bucket` extension lands in that bucket's own per-group extension pool,
   *  never the device's `schedule`/`budget` pools. */
  bucketId?: string;
}

/**
 * Contract `AppOpenRequestParams` — a child-authored ask to open, or keep
 * open past a hold's end, an app the `apps` clause presently gates via
 * `blocked`/`askFirst` (named times' "On request" policy). The GRANT
 * counterpart ([`AppOpenGrantParams`]) is a minimal answer SIGNAL — the
 * guardian's answer is that grant PLUS, only on allow, a re-signed `apps`
 * clause carrying an `AppHold`, which is what actually opens the app.
 */
export interface AppOpenRequestParams {
  /** On-device app identity — same vocabulary as appRules/buckets. */
  pkg: string;
  /** Display-only, bounded like `reason` (mirrors InstallApkRequestParams). */
  label?: string;
  /** u16, 1..=1440 when present. */
  minutesRequested?: number;
  /** ≤280 chars, private — never appears in audit/logs. */
  reason?: string;
}

/**
 * `app.open` GRANT params — a minimal answer SIGNAL, shaped exactly like
 * `TimeExtendGrantParams` (fixed 2026-08-03, review round 1: the original
 * empty-object shape meant `Decision::Allow` could never verify on the ward —
 * `verify_grant` unconditionally parses op-specific params before any
 * signature/id check runs, so "no params to parse" and "forged grant" were
 * indistinguishable, and a real allow left the pending ask stuck forever. See
 * `core/crates/charter-proto`'s `AppOpenGrantParams` and `charter-spine`'s
 * `AppOpenEnactor`, a deliberate no-op enactor). `pkg` is the TRUSTED
 * SIGNER's own echo of the REQUEST's `pkg` — by convention, same as every
 * other op's params on this wire: `verify_grant` binds `reqId`/`nonce`/`op`
 * to the pending request but never cross-checks `pkg` against it (no op
 * does — the ward keeps no copy of a pending request's params to compare
 * against). The signed GRANT is the sole authority. `minutesGranted` is the
 * answer's duration signal, `0` on a deny — it enacts nothing on its own
 * (the `AppHold` clause the guardian ALSO re-signs is what actually opens
 * the app).
 */
export interface AppOpenGrantParams {
  pkg: string;
  minutesGranted: number;
}

/** Contract `GrantPayload` — the inner content of a CHARTER_DEVICE_GRANT
 *  (31112), a FULLY-SIGNED guardian event. `reqId` + `nonce` echo the pending
 *  REQUEST verbatim; the device enacts THESE params, never the request's. */
export interface GrantPayload {
  v: 1;
  op: RequestOp;
  /** 32-byte hex, echoed verbatim from the REQUEST. */
  reqId: string;
  /** 32-byte hex, echoed verbatim from the REQUEST. */
  nonce: string;
  decision: "allow" | "deny";
  /** Unix seconds (guardian clock; device accepts within ±300s skew). */
  ts: number;
  /** Unix seconds, > ts. `time.extend`: end-of-day in the schedule tz. */
  exp: number;
  /** Per-op params (discriminated by `op`). */
  params: TimeExtendGrantParams | InstallApkGrantParams | AppOpenGrantParams;
}
