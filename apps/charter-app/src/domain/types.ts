// Kintrinsic domain model — the single source of truth for app data shapes.
// Names are load-bearing: store actions, selectors, and screens depend on them.
// Keep this file free of UI and framework concerns.

// Type-only, so the cycle with effectivePolicy.ts (which imports `Policy`
// back) is erased at compile time and never exists at runtime. The split-
// control vocabulary lives next to the logic that enforces it.
import type { PolicyOverride } from "./effectivePolicy";
// Same precedent, same reason: named times' compile/decompose logic lives
// next to the wire mapping it produces, not here.
import type { FreeGroupRecord } from "./namedTimes";

export type Weekday = "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun";

export const WEEKDAYS: Weekday[] = [
  "mon",
  "tue",
  "wed",
  "thu",
  "fri",
  "sat",
  "sun",
];

/** A single allowed time window, e.g. 16:00–18:00. "HH:MM" in 24h. */
/**
 * What happens to audio ALREADY PLAYING when the lock lands — the family's
 * agreed answer rather than a fixed one (spec 2026-07-29).
 *
 * `stop` is the behaviour that existed before this setting, and remains what an
 * absent or unusable clause means on the device: it LOOSENS enforcement, so it
 * fails closed.
 */
export interface ListeningPolicy {
  mode: "stop" | "continue" | "grace";
  /** `grace` only — minutes of listening after the lock. */
  graceMinutes?: number;
  /** Packages that count as listening. Empty means nothing is exempt. */
  apps: string[];
}

/**
 * One app open at any hour — standing, or time-boxed with an ABSOLUTE end.
 *
 * `untilUnix` is an ABSOLUTE instant, never a duration, for the same reason
 * `AppHold.untilUnix` is: a duration restarts every time the stored clause is
 * re-read and the grant would never end. Absent = standing, no end.
 */
export interface AlwaysAvailableEntry {
  pkg: string;
  untilUnix?: number;
}

/** The family's agreed list of apps open at any hour (spec 2026-08-03). */
export interface AlwaysAvailablePolicy {
  apps: AlwaysAvailableEntry[];
}

export interface ScheduleWindow {
  start: string; // "HH:MM"
  end: string; // "HH:MM"
}

/**
 * When a child is allowed to use the device/app, by day of week.
 * `overrides` keys are ISO dates ("2026-06-27") for one-off changes.
 * `paused` lifts the schedule entirely (always-allowed by schedule).
 */
export interface Schedule {
  tz: string; // IANA tz, e.g. "America/New_York"
  weekly: Partial<Record<Weekday, ScheduleWindow[]>>;
  overrides?: Record<string, ScheduleWindow[]>;
  paused?: boolean;
}

/**
 * WHAT a budget's daily/weekly allowance is spent on (2026-08-06 design,
 * "named costs — one time model, three tiers"). Mirrors the Rust
 * `charter_schedule::TimeModel` exactly — `"session"` | `"named"`.
 *
 *  - `session` (or ABSENT): today's behaviour. Being logged in costs; a live
 *    `learning` clause waives it. This is the meaning of every budget clause
 *    ever signed before this field existed, and it must STAY that meaning —
 *    see `budgetToGrant`, which emits `model` on the wire only for `"named"`.
 *  - `named`: the free-time GRANT disappears. Being at the device is free;
 *    only apps the guardian named (a `buckets`/`learning` membership) cost,
 *    they cost while a window is open, several open at once cost ONCE, and
 *    an app nobody named is free until it is.
 */
export type TimeModel = "session" | "named";

/**
 * How much total time is allowed per day/week. `null`/undefined means no cap.
 * `paused` lifts the budget entirely (no time cap).
 */
export interface Budget {
  tz: string;
  dailyMinutes?: number | null;
  weeklyMinutes?: number | null;
  weekStart?: "sun" | "mon";
  paused?: boolean;
  /** Absent = `"session"` — see `TimeModel`'s own doc for why that must never
   *  change meaning. */
  model?: TimeModel;
}

/**
 * What a policy applies to. `device` = the whole device.
 * EXPANDABILITY: `app` scopes per-app limits arriving later.
 */
export type Scope =
  | { kind: "device" }
  | { kind: "app"; appId: string; label: string };

export type WebPosture = "allowlist" | "blocklist";
export type WebAgeTier = "young" | "older";
export type YoutubeMode = "off" | "moderate" | "strict";

/**
 * Parent-authored web-content policy (v1: parent-authoritative — curators and
 * categories are deferred). Enforced on both Linux (`charterd`: Firefox
 * policy + DNS) and Android (a DO-pinned DNS-filter `VpnService`).
 * `enabled=false` LIFTS filtering (maps to the wire's `revoked`, never `paused`
 * — which would block ALL web).
 */
export interface WebPolicy {
  enabled: boolean;
  /** allowlist = only allowed sites; blocklist = everything except blocked. */
  posture: WebPosture;
  ageTier: WebAgeTier;
  /** Sites the parent always allows (domains or URL prefixes). */
  allow: string[];
  /** Sites the parent always blocks — these outrank everything. */
  block: string[];
  youtube: YoutubeMode;
  safeSearch: boolean;
}

export type AppListPosture = "blocklist" | "allowlist";

/** What an app IS for the duration of a hold. */
export type HoldState = "allowed" | "blocked";

/**
 * A time-boxed departure from the standing lists, for one app — "let him on
 * Vanadium, but just for an hour".
 *
 * `untilUnix` is an ABSOLUTE instant, never a duration, and the ward's own
 * device is what ends it. See `domain/appHolds.ts` for why nothing here may own
 * the clock.
 */
export interface AppHold {
  /** On-device identity — the same vocabulary as the standing lists. */
  pkg: string;
  state: HoldState;
  /** Unix SECONDS. The hold ends AT this instant. */
  untilUnix: number;
}

/**
 * Standing per-app policy (D3): block specific apps, or allow only a set. A
 * blocked app stays blocked even during allowed screen time. `enabled=false`
 * lifts it. Apps are identified by package name.
 */
export interface AppsPolicy {
  enabled: boolean;
  posture: AppListPosture;
  /** Blocklist: apps to block. */
  blocked: string[];
  /** Allowlist: the only apps that run. */
  allowed: string[];
  /**
   * Time-boxed departures from the lists above. Absent/empty = the standing
   * lists alone. Pruned on render and before publishing — never trusted to have
   * been tidied, because the device prunes them too.
   */
  holds?: AppHold[];
  /**
   * Apps the guardian surface should render as "Ask to open" rather than a
   * flat "Blocked" (named times' "On request" policy). A pure presentation
   * hint — enforcement reads `blocked` only. INVARIANT: every entry here must
   * also be in `blocked`; `appsToGrant` enforces this on the way to the wire.
   */
  askFirst?: string[];
  /**
   * Apps to REMOVE from the device — hidden outright, gone from the launcher,
   * the drawer and Settings as if never installed. Absent/empty = nothing
   * removed.
   *
   * Its own axis, independent of `enabled`/`posture`/`blocked`/`allowed`: a
   * tablet's OEM bloatware is not a rule about the child's day, it is junk
   * that should not be on the device at all, and (since ward 0.6.9 locks USB
   * debugging) this clause is the only way left to take it off. Lifting app
   * control does NOT put it back — see `appsToGrant`, which emits this before
   * the `paused` early-return. Reversible: drop the package and it returns.
   */
  hidden?: string[];
}

/**
 * A named set of apps with its own daily allowance — "Play is an hour a day".
 * Generalises the learning bucket: learning time is FREE, a bucket here is
 * CAPPED. Spending one closes THAT BUCKET (its apps stop), never the device.
 */
export interface AppBucketRule {
  /** Stable [a-z0-9-] slug — keys the device's day counter; never re-derived
   *  from the label, or renaming a bucket would silently reset its meter. */
  id: string;
  label: string;
  /** On-device identities (Android package id, or Linux exec path/flatpak id). */
  apps: string[];
  /** The bucket's daily allowance in minutes (1..=1440). At least one of
   *  dailyMinutes/weeklyMinutes is required; both may be set. */
  dailyMinutes?: number;
  /** The bucket's weekly allowance in minutes (1..=10080). */
  weeklyMinutes?: number;
}

/** The guardian's whole bucket set for one child. */
export interface BucketsPolicy {
  /** Off = every bucket lifted (nothing capped), without losing the set. */
  enabled: boolean;
  /** IANA tz the day boundary is computed in. */
  tz: string;
  buckets: AppBucketRule[];
  /** Week-start day for the weekly axis: 'sun' | 'mon'. Absent = 'mon' —
   *  mirrors `Budget.weekStart`. */
  weekStart?: "sun" | "mon";
}

/**
 * One learning app the guardian designated (wire-shaped). Mirrors the Rust
 * `charter_proto::LearningApp`.
 *
 * This clause does two jobs that used to be one: it DEFINES a pinned
 * site-app window (id, launch URL, verified domain closure) and, absent
 * `free`, it also GRANTS free time to whatever is in it. `free: false`
 * splits them — the window still materialises and is still pinned to its
 * closure, but the seconds are charged like any other app, with a `site:<id>`
 * identity in a `named`-model bucket/on-request group giving it its own
 * allowance (see `domain/namedTimes.ts`'s `siteIdentity`/`siteIdOf`).
 *
 * Absent/`true` is the pre-existing behaviour, unchanged — an already-signed
 * charter's every entry keeps meaning exactly what it always meant.
 */
export interface LearningAppSel {
  id: string;
  label: string;
  kind: "site" | "native";
  domains?: string[];
  url?: string;
  exec?: string;
  trusted?: boolean;
  /** Absent = free — see this interface's own doc. */
  free?: boolean;
}

/** Learning time: apps whose focused time never drains screen time. */
export interface LearningPolicy {
  enabled: boolean;
  apps: LearningAppSel[];
  /** Daily learning cap in minutes; unset = uncapped ("maths is free"). */
  capMinutes?: number;
}

/** The ward's hotspot posture. `raw` shares UNFILTERED internet with connected
 *  devices; `filtered` hosts Kintrinsic's own hotspot so guests are filtered too;
 *  `none` blocks it. `until` (unix seconds) time-boxes the allowance. */
export interface Tethering {
  allow: "none" | "raw" | "filtered";
  until?: number;
}

/** One lifeline entry: a label the ward recognises ("Mum") + the number the
 *  lock screen dials. */
export interface LifelineNumberEntry {
  label: string;
  number: string;
}

/** What a break-glass unlock opens, and for how long. */
export interface BreakGlassSettings {
  enabled: boolean;
  /** "calls" = the phone stays a phone; "full" = the whole device. */
  scope: "calls" | "full";
  durationMinutes: number;
}

/** Spec D9 — the communication lifeline: up to 5 guardian numbers the ward
 *  can ALWAYS call from the lock screen, even mid-lockout, plus the phone's
 *  own emergency number and the emergency override. A locked phone is still
 *  a phone. */
export interface Lifeline {
  numbers: LifelineNumberEntry[];
  /** Show the platform's region-correct emergency number (999/911/112/…).
   *  The DEVICE resolves the digits — we never carry them. */
  emergencyServices?: boolean;
  /** Offer a torch on the lock screen. A locked phone is still a light — a
   *  child walking home in the dark shouldn't have to break the glass to see. */
  torch?: boolean;
  breakGlass?: BreakGlassSettings;
}

/** A schedule and/or budget and/or web and/or apps policy for one child. */
export interface Policy {
  id: string;
  scope: Scope;
  schedule?: Schedule;
  budget?: Budget;
  web?: WebPolicy;
  apps?: AppsPolicy;
  learning?: LearningPolicy;
  /** Device scope only: the ward's hotspot posture (default blocked). */
  tethering?: Tethering;
  /** Device scope only: guardian numbers callable from the lock screen (D9). */
  lifeline?: Lifeline;
  /** Device scope only: named app buckets with their own daily allowance. */
  buckets?: BucketsPolicy;
  /** Device scope only: what happens to audio already playing at the lock. */
  listening?: ListeningPolicy;
  /** Device scope only: apps open at any hour, standing or expiring. */
  alwaysAvailable?: AlwaysAvailablePolicy;
  /**
   * App scope only: when true the app/game is **revoked** — blocked from
   * loading on the device. Enforced at the OS level by `charterd`; this is
   * control, not reporting (per-app usage is never sent back to the parent).
   */
  blocked?: boolean;
  /**
   * Device scope only: per-device divergence from this base charter, keyed by
   * `Device.id`. Absent (the default, and every child before this existed)
   * means one charter across all their devices.
   *
   * Kintrinsic's OWN bookkeeping — it must never reach the wire. Publishing
   * resolves it to one effective policy per device; see
   * `domain/effectivePolicy.ts`.
   */
  deviceOverrides?: Record<string, PolicyOverride>;
  /**
   * Device scope only: Named-times FREE groups' membership (id/label/apps).
   * Guardian-side bookkeeping, exactly the `deviceOverrides` precedent — a
   * free group has no identity on the wire at all (the `learning` clause is
   * just a flat app union), so this is the only place a free group's name
   * AND which apps belong to it survive a reload — including keeping two or
   * more free groups distinct, which the wire alone can never do. See
   * `domain/namedTimes.ts`.
   */
  freeGroups?: FreeGroupRecord[];
}

/** Where a device is in the parent-governed pairing handshake. */
export type PairingState = "unpaired" | "pairing" | "paired";

/**
 * A device governed by the parent. Stage 0 wedge: time limits land on a
 * paired device. Requests originate FROM the device (see ChildRequest), and
 * the device carries its OWN key (`devicePubkey`) — governed by the parent,
 * not by any child account. Crypto is mocked for now; the FLOW is real.
 *
 * Pairing direction: the laptop shows a QR + short code; Kintrinsic (the
 * phone) scans it and binds it ("Govern this device").
 */
export interface Device {
  id: string;
  label: string; // "Sam's laptop"
  platform: "linux" | "android"; // charterd computer, or Kintrinsic-managed phone
  pairing: PairingState;
  pairedAt?: number; // epoch ms
  devicePubkey?: string | null; // the DEVICE's own key — mock hex for now
  lastSeenAt?: number; // epoch ms
}

export interface Child {
  id: string;
  name: string;
  color: string; // hex accent for this child's chips/avatars
  /**
   * Stage 0 is ALWAYS null — a child is a LOCAL LABEL in Kintrinsic, with no
   * Signet account. Stage 2 seam: adding the child's Signet persona later
   * sets this and unlocks game/app logins. No Stage 0/1 code writes it.
   */
  dependantPubkey: string | null;
  /** One or more governed devices. Replaces the old `devicePaired` flag. */
  devices: Device[];
  policies: Policy[];
}

/**
 * Who a request comes from. Stage 0/1 is ALWAYS "device" (the OS said no, the
 * child asked on the device). "persona" is the Stage 2 seam — not produced by
 * any Stage 0/1 code.
 */
export type Requester = "device" | "persona";

/** Kinds of things a child can ask a parent to allow. `app.open` (named
 *  times' "ask to open") is answered differently from the others — see
 *  `wire/grant.ts`'s `buildAppOpenGrant` and `domain/appHolds.ts`. */
export type RequestKind = "time.extend" | "install.app" | "run.program" | "app.open";

export interface ChildRequest {
  id: string;
  childId: string;
  /** Stage 0/1 always "device"; "persona" reserved for Stage 2. */
  requester: Requester;
  /** Which device originated the ask (set when requester === "device"). */
  deviceId?: string;
  kind: RequestKind;
  createdAt: number; // epoch ms
  title: string;
  reason?: string;
  minutesRequested?: number;
  /**
   * The guardian's own chosen grant amount, when they've stepped it down from
   * `minutesRequested` — persisted on the request record itself (not local
   * component state) so it survives a failed send + remount. Before this
   * (M-1, hardware round 2026-08-03) the stepper lived in a screen-local
   * `useState`, which reset to the FULL requested amount whenever Approvals
   * remounted (the exact path a failed send forces: fail → go fix → come
   * back → approve) — a guardian who dialled 30 down to 5 could have their
   * retry silently grant 30. Absent = "hasn't touched the stepper yet",
   * still defaulting to `minutesRequested`.
   */
  chosenMinutes?: number;
  limitHit?: "schedule" | "budget" | "bucket";
  /** Present only when `limitHit === 'bucket'` — the specific named-times
   *  group asked about. Echoed verbatim into the grant. */
  bucketId?: string;
  /** Stage 1: the SPECIFIC artifact, e.g. "VLC media player". */
  appLabel?: string;
  /** Stage 1: optional artifact ref/hash, e.g. "flatpak:org.videolan.VLC". */
  appId?: string;
  /**
   * "dismissed" (approvals-clarity, 2026-08-04): the guardian let a repeat
   * ask go without answering it — local-only, never signed, never told to
   * the ward. Deliberately a `status`, not a deletion: the record (and its
   * `reqId`, for a real device-originated ask) stays in `requests` so the
   * relay-poll dedupe in `store.tsx`'s `ingestDeviceRequest` — which matches
   * on `reqId` against the WHOLE list, any status — keeps refusing to
   * re-add it once the 24h relay window turns the same ask up again.
   */
  status: "pending" | "approved" | "denied" | "dismissed";

  // --- Wire correlation (real device-broker intake only) -----------------
  // Set when the request arrived over the relay from a real device (kind 31111
  // via gift-wrap); ALL absent on simulated/demo requests, whose approval is
  // purely local (no GRANT is published).
  /** The REQUEST's reqId (32-byte hex) — echoed verbatim in the signed GRANT. */
  reqId?: string;
  /** The REQUEST's single-use nonce (32-byte hex), echoed in the GRANT. */
  nonce?: string;
  /** The asking device's pubkey (hex) — the GRANT's gift-wrap recipient. */
  machine?: string;
  /** The managed user's pubkey (hex) — who asked (== contract `subject`). */
  subject?: string;
}

export type ActivityOutcome =
  | "enacted"
  | "denied"
  | "failed"
  | "locked"
  | "thawed"
  | "rule-changed"
  /** The ward used their emergency unlock (break-glass). Not a failure and
   *  not a breach — a visible event to talk about. */
  | "override";

export interface ActivityEvent {
  id: string;
  childId: string;
  ts: number; // epoch ms
  outcome: ActivityOutcome;
  summary: string;
  /**
   * Present only for a named-times EXTEND or GIFT that credited one specific
   * group — the raw wire bucket id, joined the same way `groupProgress.ts`
   * joins STATUS. Absent for a whole-device gift/extend (nothing to attribute
   * the extra to) and for every non-grant activity kind.
   */
  bucketId?: string;
  /**
   * The minutes this event actually credited, when `bucketId` is set — the
   * guardian's own record of what THEY granted, used to keep the Today
   * card's displayed CAP honest (M-2, hardware round 2026-08-03): STATUS's
   * raw meters correctly show what the device spent, but a cap join that
   * only knows the BASE `dailyMinutes`/`weeklyMinutes` reads a spend inside a
   * grant the guardian personally approved as a breach that never happened.
   * See `domain/groupProgress.ts`'s `groupExtrasToday`.
   */
  minutesGranted?: number;
}

/** The signing helper the parent uses to authorize changes/decisions. */
export interface SignerState {
  connected: boolean;
  kind: "none" | "signet" | "heartwood" | "local";
  autoSign: boolean;
  label?: string;
  /**
   * When set, the signer is the REAL Signet bunker reachable at this
   * `bunker://…` URI (persisted, so a paired session survives reload). Absent =
   * the local mock signer (the demo default).
   */
  bunkerUri?: string;
}

export type SignerKind = SignerState["kind"];
