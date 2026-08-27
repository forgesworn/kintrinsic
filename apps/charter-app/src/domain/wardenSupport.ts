// Can the ward's devices actually honour what the guardian is about to send?
//
// One place for a rule that was hand-rolled three times (gift, stand-down,
// v2 lifeline) and got it subtly wrong each time. The wrong version of the
// rule treated a device that had NOT REPORTED as incapable, which greyed out
// Finish now for a ward whose laptop was simply switched off (decented,
// 2026-07-28) — refusing an action for the very reason it exists.
//
// A clause is durable: the relay holds it and the device applies it when it
// next connects (and a stand-down lapses at the ward's midnight, so a device
// that stays off all evening is never surprise-locked tomorrow). So:
//
//   silence is not incapacity.
//
// Only a device that ACTUALLY REPORTS a Kintrinsic older than the feature blocks
// anything — and even then only the devices that reported old are named. The
// action is refused outright only when nothing paired could honour it, which
// is the one case where the button would be a lie.

import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";

/** Guardian actions that need a warden new enough to understand them. */
export type WardenFeature =
  | "standdown"
  | "gift"
  | "lifelineV2"
  | "appHold"
  | "appHide"
  // Named times (2026-08): the weekly bucket axis, and the app.open ask /
  // askFirst "on request" affordance. Both are new to THIS branch's wards on
  // BOTH platforms, unlike the older per-platform-only gates below.
  | "bucketsWeekly"
  | "appOpenAsk"
  // Clause dimensions one warden enforces and the other does not — see NEVER.
  | "learning"
  | "buckets"
  | "listening"
  | "lifeline"
  | "tethering"
  | "alwaysAvailable"
  // Honest attribution (2026-08-03): the `cmdline:` identity form and the
  // unrecognised-time counter are both Linux-only concepts by construction —
  // Android has no root-owned-interpreter ambiguity to name a process by its
  // own command line, and no notion of an "unvouched" exe owner (every
  // installed APK is signed and reported through DevicePolicyManager, never a
  // ward-writable binary a launcher shells out to). NEVER, not a version
  // threshold: no future Android build will ever grow either.
  | "cmdlineIdentity"
  | "unrecognisedTime"
  // The `named` time model (2026-08-06 design "named costs — one time
  // model, three tiers"): only apps the guardian named cost, and being at
  // the device is otherwise free. Linux-only for now — Android's foreground-
  // package reading of the same model is a later pass (the design doc's own
  // §3.3 build order), not yet landed on any ward build.
  | "timeModel";

/**
 * A platform whose warden does not enforce this clause AT ALL — no version of
 * it ever has. Distinct from "too old", and the distinction is the whole point:
 * an old warden is a device that will honour the rule once it updates, so the
 * clause is worth sending and silence means "wait". A platform that never reads
 * the clause will never honour it, so a guardian who is not told is authoring a
 * rule that does not exist. Version thresholds are also useless here — the
 * device reports a perfectly current Kintrinsic and still ignores the clause.
 *
 * The 2026-08-02 audit found six of fourteen clauses in this state with nothing
 * anywhere saying so. Keep this table honest against `spec/contract.md`'s
 * "Enforcement parity" section; the two are meant to say the same thing.
 */
export const NEVER = "unsupported" as const;
type Threshold = number | typeof NEVER;

/**
 * The lowest version of each warden that understands a feature, per platform.
 * Android counts APK versionCodes; charterd counts major*10000 + minor*100 +
 * patch (0.4.0 = 400). `undefined` means "no known threshold" — every shipped
 * version of that warden understands it, so nothing is ever blocked on it.
 * [`NEVER`] means that platform does not enforce it at any version.
 */
const MIN_VERSION: Record<WardenFeature, Partial<Record<Device["platform"], Threshold>>> = {
  // Ward Kintrinsic 0.5.0 (24); charterd 0.4.0 (400) — both shipped 2026-07-27.
  standdown: { android: 24, linux: 400 },
  // Ward Kintrinsic 0.4.0 (22); charterd gained it in the 0.4.0 build too.
  gift: { android: 22, linux: 400 },
  // Ward Kintrinsic 0.3.2 (14). charterd has no lock-screen lifeline.
  lifelineV2: { android: 14, linux: NEVER },
  // charterd stores the lifeline clause and never reads it: no call button, no
  // torch, no break-glass on the Linux lock. A laptop-only ward has no
  // lock-screen way to reach anyone.
  lifeline: { linux: NEVER },
  // The learning bucket (time-free homework apps) is Linux-only; `android/jni`
  // never references ClauseKind::Learning.
  learning: { android: NEVER },
  // Named daily allowances ("Play is an hour a day") shipped Linux-only in
  // 0.3.0; Android gained buckets enforcement in the named-times branch,
  // ward Kintrinsic 0.6.7 (versionCode 38) — the release straight after the
  // 0.6.6 (37) build this repo shipped before named times.
  buckets: { android: 38 },
  // The weekly axis ("Play is five hours a week") — new on BOTH platforms in
  // the named-times branch: ward Kintrinsic 0.6.7 (38), charterd 0.7.3 (703),
  // the releases straight after this repo's pre-named-times build (ward 37,
  // charterd 0.7.2). A stale ward on either platform silently falls back to
  // reading nothing from a weekly-only (`v: 2`) bucket set — the buckets
  // fail-open doctrine — so this gate is what lets the guardian surface warn
  // about that BEFORE sending, rather than after the fact.
  bucketsWeekly: { android: 38, linux: 703 },
  // "Ask to open" (askFirst hint + the app.open brokered ask) — likewise new
  // on both platforms in the same branch and releases as bucketsWeekly.
  appOpenAsk: { android: 38, linux: 703 },
  // Audio surviving the lock is Android-only: a Linux lock freezes the whole
  // user slice, so audio stops regardless of what the clause says.
  listening: { linux: NEVER },
  // Always-available apps ("open at any hour, even mid-lock") are Android
  // only, for the SAME architectural reason as `listening` directly above:
  // charterd's lock freezes the child's whole user slice indiscriminately
  // (`enforcer_runtime.rs`), so a named app would be frozen along with
  // everything else regardless of what this clause says — there is no
  // per-process exemption to freeze around. Ward Kintrinsic 0.6.8 (versionCode
  // 39) is the first build that reads the clause at all.
  alwaysAvailable: { android: 39, linux: NEVER },
  // A laptop has no hotspot for Kintrinsic to govern.
  tethering: { linux: NEVER },
  // Ward Kintrinsic 0.6.5 (36); charterd 0.7.1 (701) — before 0.7.1 charterd never
  // read the `apps` clause at all (only `appRules`), so on a laptop this
  // threshold gates the standing toggles AND holds alike: an older charterd
  // ignores the whole clause, and naming that honestly ("needs the latest
  // Kintrinsic") is exactly what this gate is for.
  appHold: { android: 36, linux: 701 },
  // "Remove from device" — hiding an app outright (`apps.hidden`). Ward
  // Kintrinsic 0.6.10 (versionCode 41) is the first build that reads the field;
  // 0.6.9 (40) is the release that locked USB debugging and so created the
  // need for it. charterd never hides apps at any version: there is no Linux
  // equivalent of a Device Owner making a package vanish from the launcher,
  // and a laptop has no OEM bloat a guardian can't uninstall the ordinary
  // way — so NEVER, not a future threshold this repo hasn't shipped.
  appHide: { android: 41, linux: NEVER },
  // Honest attribution (charterd 0.7.5 / versionCode 705): the `cmdline:`
  // identity form, and the `unrecognisedTodaySecs` floor it makes visible
  // for. Android never enforces either (see the union's own doc above).
  cmdlineIdentity: { android: NEVER, linux: 705 },
  unrecognisedTime: { android: NEVER, linux: 705 },
  // charterd 0.7.6 (706) — the release straight after honest attribution
  // (705), first build that reads `budget.model`. Android support is a
  // deliberate later pass (design doc §8 build order item 2), so it is
  // gated `NEVER` for now rather than a future versionCode this repo hasn't
  // shipped yet — `NEVER` is honestly what every Android build up to and
  // including this one is, and the day Android lands it this line becomes a
  // real threshold, not a re-interpretation of an old one.
  timeModel: { android: NEVER, linux: 706 },
};

/** Every paired device a clause would actually be sealed to. */
export function deliverableDevices(devices: Device[]): Device[] {
  return devices.filter((d) => d.pairing === "paired" && Boolean(d.devicePubkey));
}

export interface WardenSupport {
  /** Whether the action is worth offering at all. */
  canSend: boolean;
  /** Devices that REPORT a Kintrinsic too old for this — never merely quiet ones. */
  tooOld: Device[];
  /** Devices whose platform never enforces this, at any version (see NEVER). */
  unsupported: Device[];
}

export function wardenSupport(
  devices: Device[],
  deviceStatus: Record<string, DeviceStatus>,
  feature: WardenFeature,
): WardenSupport {
  const thresholds = MIN_VERSION[feature];
  // Platform, unlike version, is known from the device record itself — no
  // report needed — so "silence is not incapacity" does not apply here. A
  // laptop is a laptop whether or not it has phoned home today.
  const unsupported = devices.filter((d) => thresholds[d.platform] === NEVER);
  const tooOld = devices.filter((d) => {
    const min = thresholds[d.platform];
    if (min == null || min === NEVER) return false;
    const reported = deviceStatus[d.devicePubkey as string]?.appVersionCode;
    // Not heard from = assumed capable. The clause waits for it.
    if (reported == null) return false;
    return reported < min;
  });
  const beyond = new Set([...tooOld, ...unsupported]);
  return {
    canSend: devices.length === 0 || beyond.size < devices.length,
    tooOld,
    unsupported,
  };
}

/**
 * The sentence to put under a control when some of the ward's devices will
 * never honour it — or `null` when every one of them can. Named devices, never
 * a count: "this does nothing on one of your devices" sends a parent hunting.
 *
 * Deliberately plain about the consequence ("does nothing there") rather than
 * hedging, and it says what DOES govern that device where there is an honest
 * answer, so the guardian's next move is obvious.
 */
export function unsupportedNote(
  support: WardenSupport,
  what: string,
  insteadOnThose?: string,
): string | null {
  if (support.unsupported.length === 0) return null;
  const names = support.unsupported.map((d) => d.label).join(" and ");
  const tail = insteadOnThose ? ` ${insteadOnThose}` : "";
  return `${what} does nothing on ${names}.${tail}`;
}

/**
 * The sentence to put under a control when some of the ward's devices merely
 * report a Kintrinsic TOO OLD for it — or `null` when none do. Distinct from
 * `unsupportedNote`'s permanent platform gap: this device WILL honour the
 * clause once it updates, so silence here means "wait", never "never" (see
 * this module's own `NEVER` doc). Named devices, never a count, for the same
 * reason `unsupportedNote` never counts — and states plainly what to do about
 * it, since (unlike the never-supported case) there is something to do.
 *
 * Wording mirrors the app-hold sheet's and the `cmdlineIdentity` note's own
 * hand-rolled "needs the latest Kintrinsic before it can…" sentence — pulled out
 * here so a THIRD caller (always-available) doesn't hand-roll a fourth
 * variant, and so the sentence is directly testable against `wardenSupport`'s
 * own fixtures rather than only indirectly through a screen render.
 */
export function tooOldNote(support: WardenSupport, what: string): string | null {
  if (support.tooOld.length === 0) return null;
  const names = support.tooOld.map((d) => d.label).join(" and ");
  return `${names} needs the latest Kintrinsic before it can ${what}. Update it, then this starts working by itself.`;
}
