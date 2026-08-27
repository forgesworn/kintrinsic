// Consuming the device STATUS feed (kind 31114) — the reverse of what
// `charterd` emits (`spec/contract.md` "Device STATUS feed"). Like REQUEST, the
// STATUS rumor is UNSIGNED on the wire, but the machine's schnorr signature
// lives on the SEAL (kind 13): the device seals STATUS with its own machine key
// (`build_status_wrap` → `nip59::wrap(rumor, machine_sk, ...)`), so authenticity
// = a valid machine-signed seal PLUS the author binding
// rumor.pubkey == seal.pubkey == payload.machine — the same binding charterd's
// `nip59::unwrap` enforces in the other direction. nip44 sealing ALONE is not
// authentication (the conversation key is symmetric ECDH, so anyone can encrypt
// a payload to the guardian's public key and self-declare any author), which is
// why the seal signature must be verified. The payload is numbers + enums
// only — never any child PII.

import { nip44, verifyEvent, type NostrEvent } from "nostr-tools";
import { decodeMinutes } from "../insights/minuteSet";
import { MAX_WRAP_JITTER_SECS } from "./request";

export const CHARTER_DEVICE_STATUS = 31114;
const SEAL = 13;

export type StatusSource = "guardian" | "device-only" | "unconstrained";
export type StatusLockReason = "schedule" | "budget" | "malformed" | "standdown";

/** Contract `StatusPayload` — one child's live state on one device. */
export interface DeviceStatus {
  v: 1;
  /** The child (== Signet dependantId / subject pubkey hex). */
  subject: string;
  /** The device this came from (== Pairing.machine pubkey hex). */
  machine: string;
  /** Unix SECONDS (device clock). */
  ts: number;
  dayKey: string;
  weekKey?: string;
  usedTodaySecs: number;
  /** Learning-bucket seconds today (present when a learning clause is in force). */
  learningTodaySecs?: number;
  /** True when the child has educational site apps in force but the device has
   *  no Chromium-family runtime to host them, so none of their windows can
   *  open. Reported because the alternative is silence: the device retries
   *  forever, so a guardian ticked a site and simply nothing happened. */
  siteRuntimeMissing?: boolean;
  /** This device's active-minutes journal today — MinuteSet base64url (240
   *  chars), the union-rule input (contract §USAGE_SYNC extension). Only ever
   *  set when it strictly decodes; malformed bitmaps are dropped at parse. */
  activeMinutesToday?: string;
  usedWeekSecs?: number;
  /** The limits actually in force on that device, so we can SHOW them rather
   *  than a blank. Limits set on the machine itself were invisible here, which
   *  read as "not selected" while the device enforced two hours. Read together
   *  with `source` to say both the rule and who set it. */
  dailyMinutes?: number;
  weeklyMinutes?: number;
  /**
   * Per-bucket spent seconds, day- and week-keyed (named times). Absent until
   * a `buckets` clause is in force; capped at 12 entries (mirrors
   * `MAX_BUCKETS`) — a payload claiming more is TRUNCATED on parse, not
   * rejected, since losing an otherwise-trustworthy STATUS over a malformed
   * tail would hide real numbers the guardian is owed. Never per-app detail —
   * that is where reflection tips into monitoring.
   */
  groups?: StatusGroup[];
  windowLeftSecs: number;
  quotaLeftSecs: number;
  effectiveSecs: number;
  locked: boolean;
  lockReason?: StatusLockReason;
  source: StatusSource;
  /** The device's Kintrinsic app versionCode — update state (#44). */
  appVersionCode?: number;
  /** The device's version as humans write it ("0.3.6"). Shown to the guardian;
   *  `appVersionCode` is what comparisons use. */
  appVersionName?: string;
  /** Set when a Kintrinsic update keeps failing — the guardian's only signal that
   *  the phone is stuck rather than merely slow. */
  updateHealth?: {
    packageName: string;
    versionCode?: number;
    attempts: number;
    sinceUnix: number;
    lastError?: string;
  };
  /**
   * An account of the last install window the guardian opened: when the phone
   * saw it open, when it shut, and what came through it. Scoped to the window
   * and nothing else — Kintrinsic reports what a loosening let through, never a
   * running feed of what a child installs.
   */
  installWindow?: {
    startedAt: number;
    endedAt?: number;
    changes: {
      pkg: string;
      label: string;
      kind: "installed" | "updated";
      at: number;
    }[];
  };
  /**
   * One-time QR-onboarding echo: the token from the pairing QR this device
   * scanned, present only for a short window after a fresh pairing. Matching
   * it against the outstanding QR is how the device gets bound without the
   * parent typing its code.
   */
  pairToken?: string;
  /** The device's installed launchable apps (package + label) — the guardian's
   *  app-picker source for per-app control (D3). Absent until reported. */
  apps?: AppRef[];
  /**
   * Foreground-screen seconds today that are not vouched for (a known,
   * non-root exe owner) AND not matched by any identity any of the child's
   * clauses currently name (charterd >= 0.7.5, honest attribution). Absent
   * when zero or unknown — a device that has never seen anything
   * unrecognised stays byte-identical to a pre-0.7.5 payload, and "absent"
   * must never be coerced to 0: that would claim a clean bill of health from
   * a device that simply hasn't reported.
   *
   * SCOPE — this is a FLOOR, not a total (contract's binding rule): a
   * ward-authored payload run by a root-owned interpreter (`java -jar
   * ~/x.jar`) is deliberately never counted, because that shape is
   * indistinguishable from a system app opening the child's own document.
   * No surface may present this number as a complete account of unvouched
   * software.
   */
  unrecognisedTodaySecs?: number;
  /**
   * Seconds spent, TODAY, in an `alwaysavailable` app while the device was
   * LOCKED (charterd/ward >= 2026-08-03). Absent when zero or unknown, never
   * coerced to 0. Android-only: the always-available clause that produces it
   * doesn't exist on Linux — the mirror image of `unrecognisedTodaySecs`,
   * which is Linux-only.
   *
   * CARRIED, NOT YET DISPLAYED (M1, review 2026-08-04): credited, parsed and
   * tested, but no surface reads it — the week-keyed pair below is what the
   * guardian's and ward's weekly lines actually render. Kept for a future
   * PER-DAY out-of-hours account ("just tonight") that will want this
   * already flowing over the wire.
   */
  outOfHoursTodaySecs?: number;
  /** Week-keyed twin of `outOfHoursTodaySecs`, for the weekly line ("3
   *  nights so far this week · 2h 15m") — see `outOfHoursLine` in
   *  `insights/usageHistory.ts`. Rolls with `usedWeekSecs`, not the day. */
  outOfHoursWeekSecs?: number;
  /** Count of distinct days this week that saw any out-of-hours use — the
   *  "3 nights" half of `outOfHoursLine`. */
  outOfHoursNightsWeek?: number;
  /**
   * Boots the device went through with NO warden running (ward >= 2026-08-07).
   * Absent when there have been none, which is the ordinary state.
   *
   * Android safe mode disables every third-party package, a Device Owner
   * included, so a safe-mode session leaves no tick, no lock and no report —
   * the phone comes back enforcing as though it never happened. The device
   * cannot witness its own absence, but it can count boots, and this is the
   * count of the ones it missed.
   *
   * A FLOOR and a fact, not an accusation: a flash, a restore or a
   * platform-level force-stop shows up here too. The honest reading is "the
   * warden was not running for N boots".
   */
  enforcementGap?: { unexplainedBoots: number; lastNoticedAt: number };
}

/** One installed launchable app on a device. */
export interface AppRef {
  pkg: string;
  label: string;
  /**
   * true only for an entry from a ward-writable scan dir (the managed
   * user's own `~/.local/share/applications` or `--user` flatpak exports,
   * charterd >= 0.7.5); absent (root-owned) otherwise. Flagging is what
   * makes SHOWING a ward-writable entry safe: the guardian is told plainly
   * it came from the ward's own writable area, never presented as if it
   * were an inventory the ward has no way to have altered.
   */
  userInstalled?: boolean;
  /**
   * true when the device is currently HIDING this app on the guardian's
   * orders (the `apps` clause's `hidden` list) — absent otherwise, never
   * coerced to `false`, matching `userInstalled` directly above.
   *
   * The device keeps REPORTING a hidden app rather than dropping it from the
   * inventory, and that is the whole point: an app that vanished from both
   * the tablet and the guardian's list would be unrecoverable — nothing left
   * anywhere to press "Put back" on.
   */
  hidden?: boolean;
}

/** One named-times bucket's spent seconds, day- and week-keyed. */
export interface StatusGroup {
  id: string;
  daySecs: number;
  weekSecs: number;
}

/** Cap mirrors `charter-schedule::MAX_BUCKETS`. */
const MAX_GROUPS = 12;

/** Validate the STATUS `groups` progress list. TRUNCATES to the cap rather
 *  than rejecting the whole payload on an over-long or partly-malformed tail. */
function parseGroups(v: unknown): StatusGroup[] | undefined {
  if (!Array.isArray(v)) return undefined;
  const out: StatusGroup[] = [];
  for (const e of v.slice(0, MAX_GROUPS)) {
    if (e && typeof e === "object") {
      const m = e as Record<string, unknown>;
      if (typeof m.id === "string" && m.id.length > 0 && isNonNegInt(m.daySecs) && isNonNegInt(m.weekSecs)) {
        out.push({ id: m.id, daySecs: m.daySecs, weekSecs: m.weekSecs });
      }
    }
  }
  return out.length ? out : undefined;
}

/** Validate the STATUS `apps` inventory (drop malformed entries; cap length). */
function parseApps(v: unknown): AppRef[] | undefined {
  if (!Array.isArray(v)) return undefined;
  const out: AppRef[] = [];
  for (const e of v.slice(0, 500)) {
    if (e && typeof e === "object") {
      const pkg = (e as Record<string, unknown>).pkg;
      const label = (e as Record<string, unknown>).label;
      if (typeof pkg === "string" && pkg && typeof label === "string") {
        const userInstalled = (e as Record<string, unknown>).userInstalled;
        const hidden = (e as Record<string, unknown>).hidden;
        out.push({
          pkg,
          label: label || pkg,
          ...(userInstalled === true ? { userInstalled: true as const } : {}),
          ...(hidden === true ? { hidden: true as const } : {}),
        });
      }
    }
  }
  return out.length ? out : undefined;
}

const SOURCES: StatusSource[] = ["guardian", "device-only", "unconstrained"];
const REASONS: StatusLockReason[] = ["schedule", "budget", "malformed", "standdown"];

function isHex(s: unknown): s is string {
  return typeof s === "string" && /^[0-9a-f]{64}$/.test(s);
}
function isNonNegInt(n: unknown): n is number {
  return typeof n === "number" && Number.isFinite(n) && n >= 0;
}

/**
 * Parse + validate a STATUS payload from a rumor's `content` JSON. Returns
 * `null` on any malformed field — a bad feed is dropped, never trusted.
 */
export function parseStatus(json: string): DeviceStatus | null {
  let o: Record<string, unknown>;
  try {
    o = JSON.parse(json) as Record<string, unknown>;
  } catch {
    return null;
  }
  if (o.v !== 1) return null;
  if (!isHex(o.subject) || !isHex(o.machine)) return null;
  if (!isNonNegInt(o.ts)) return null;
  if (typeof o.dayKey !== "string") return null;
  if (
    !isNonNegInt(o.usedTodaySecs) ||
    !isNonNegInt(o.windowLeftSecs) ||
    !isNonNegInt(o.quotaLeftSecs) ||
    !isNonNegInt(o.effectiveSecs)
  ) {
    return null;
  }
  if (typeof o.locked !== "boolean") return null;
  if (!SOURCES.includes(o.source as StatusSource)) return null;
  // An unrecognised lock reason DEGRADES; it does not invalidate the heartbeat.
  // Rejecting the whole payload made every future clause kind a trap: a phone on
  // newer firmware reporting a reason this build has never heard of would vanish
  // from the guardian's view entirely — no last-seen, no time-left — and read as
  // a dead phone rather than a new word. The field is display-only, and the rest
  // of the report is still perfectly good.
  const pairToken =
    typeof o.pairToken === "string" && /^[0-9a-zA-Z_-]{8,128}$/.test(o.pairToken)
      ? o.pairToken
      : undefined;
  return {
    v: 1,
    subject: o.subject,
    machine: o.machine,
    ts: o.ts,
    dayKey: o.dayKey,
    weekKey: typeof o.weekKey === "string" ? o.weekKey : undefined,
    usedTodaySecs: o.usedTodaySecs,
    learningTodaySecs: isNonNegInt(o.learningTodaySecs) ? o.learningTodaySecs : undefined,
    siteRuntimeMissing: o.siteRuntimeMissing === true ? true : undefined,
    activeMinutesToday:
      typeof o.activeMinutesToday === "string" && decodeMinutes(o.activeMinutesToday)
        ? o.activeMinutesToday
        : undefined,
    usedWeekSecs: isNonNegInt(o.usedWeekSecs) ? o.usedWeekSecs : undefined,
    dailyMinutes: isNonNegInt(o.dailyMinutes) ? o.dailyMinutes : undefined,
    weeklyMinutes: isNonNegInt(o.weeklyMinutes) ? o.weeklyMinutes : undefined,
    groups: parseGroups(o.groups),
    appVersionCode: isNonNegInt(o.appVersionCode) ? o.appVersionCode : undefined,
    appVersionName:
      typeof o.appVersionName === "string" && o.appVersionName.length > 0 && o.appVersionName.length <= 32
        ? o.appVersionName
        : undefined,
    updateHealth: parseUpdateHealth(o.updateHealth),
    installWindow: parseInstallWindow(o.installWindow),
    windowLeftSecs: o.windowLeftSecs,
    quotaLeftSecs: o.quotaLeftSecs,
    effectiveSecs: o.effectiveSecs,
    unrecognisedTodaySecs: isNonNegInt(o.unrecognisedTodaySecs) ? o.unrecognisedTodaySecs : undefined,
    outOfHoursTodaySecs: isNonNegInt(o.outOfHoursTodaySecs) ? o.outOfHoursTodaySecs : undefined,
    outOfHoursWeekSecs: isNonNegInt(o.outOfHoursWeekSecs) ? o.outOfHoursWeekSecs : undefined,
    outOfHoursNightsWeek: isNonNegInt(o.outOfHoursNightsWeek) ? o.outOfHoursNightsWeek : undefined,
    enforcementGap: parseEnforcementGap(o.enforcementGap),
    locked: o.locked,
    lockReason: REASONS.includes(o.lockReason as StatusLockReason)
      ? (o.lockReason as StatusLockReason)
      : undefined,
    source: o.source as StatusSource,
    pairToken,
    apps: parseApps(o.apps),
  };
}

/**
 * Unwrap a gift-wrapped (1059) STATUS the device sealed to the guardian:
 * decrypt wrap → seal → rumor with the guardian secret key, AUTHENTICATE the
 * machine (the seal's schnorr sig — the only device signature on this wire —
 * plus rumor.pubkey == seal.pubkey == payload.machine, so nobody can attribute
 * a forged status to a device whose key they don't hold), confirm the rumor is
 * a kind-31114 charter STATUS, and parse it. The rumor's author (== the seal
 * author) is the emitting DEVICE — returned so the PWA can attribute per device.
 * Returns `null` on any decrypt / verify / kind / freshness / parse failure
 * (never throws).
 */
export function unwrapStatus(
  wrap: NostrEvent,
  recipientSk: Uint8Array,
  nowSecs: number = Math.floor(Date.now() / 1000),
): DeviceStatus | null {
  try {
    // Jitter: wraps are backdated; drop one too far from now in either direction
    // (the same window charterd's nip59::unwrap enforces on its side).
    if (Math.abs(nowSecs - wrap.created_at) > MAX_WRAP_JITTER_SECS) return null;
    const seal: NostrEvent = JSON.parse(
      nip44.decrypt(wrap.content, nip44.getConversationKey(recipientSk, wrap.pubkey)),
    );
    if (seal.kind !== SEAL) return null;
    // The seal is the machine's SIGNED event — verify it (id + schnorr sig).
    // Without this, nip44 decryption alone would accept a payload sealed by ANY
    // keypair the attacker controls, self-declaring an arbitrary author.
    if (!verifyEvent(seal)) return null;
    const rumor: NostrEvent = JSON.parse(
      nip44.decrypt(seal.content, nip44.getConversationKey(recipientSk, seal.pubkey)),
    );
    if (rumor.kind !== CHARTER_DEVICE_STATUS) return null;
    // The sealed (signed) author must equal the rumor author — no forged sender.
    if (rumor.pubkey !== seal.pubkey) return null;
    const status = parseStatus(rumor.content);
    // The rumor author is the device; the payload's `machine` must match it, so a
    // relay can't attribute one device's status to another.
    if (status && status.machine !== rumor.pubkey) return null;
    return status;
  } catch {
    return null;
  }
}

/**
 * Fail-quiet like the rest: an unparseable account is "no signal". Deliberately
 * NOT "no changes" — showing a confident "nothing was installed" off a block we
 * failed to read would be the one wrong answer here, because it is the answer a
 * guardian would act on.
 *
 * Capped at 200 entries. A window is at most an hour, so a longer list is a
 * malformed or hostile payload rather than a real account.
 */
/**
 * A zero count is NOT a gap — it is the ordinary state, and the sheet must not
 * grow a "0 unwarded boots" line the moment a device reports the field at all.
 * Dropped rather than coerced, exactly like the other absent-not-zero meters.
 */
function parseEnforcementGap(o: unknown): DeviceStatus["enforcementGap"] {
  if (typeof o !== "object" || o === null) return undefined;
  const m = o as Record<string, unknown>;
  if (!isNonNegInt(m.unexplainedBoots) || m.unexplainedBoots === 0) return undefined;
  if (!isNonNegInt(m.lastNoticedAt)) return undefined;
  return { unexplainedBoots: m.unexplainedBoots, lastNoticedAt: m.lastNoticedAt };
}

function parseInstallWindow(o: unknown): DeviceStatus["installWindow"] {
  if (typeof o !== "object" || o === null) return undefined;
  const m = o as Record<string, unknown>;
  if (!isNonNegInt(m.startedAt)) return undefined;
  if (!Array.isArray(m.changes)) return undefined;
  const changes = m.changes
    .slice(0, 200)
    .map((c) => {
      if (typeof c !== "object" || c === null) return null;
      const e = c as Record<string, unknown>;
      if (typeof e.pkg !== "string" || e.pkg.length === 0 || e.pkg.length > 255) return null;
      if (e.kind !== "installed" && e.kind !== "updated") return null;
      if (!isNonNegInt(e.at)) return null;
      const label =
        typeof e.label === "string" && e.label.length > 0 && e.label.length <= 100
          ? e.label
          : e.pkg;
      const kind: "installed" | "updated" = e.kind;
      return { pkg: e.pkg, label, kind, at: e.at };
    })
    .filter((c): c is NonNullable<typeof c> => c !== null);
  return {
    startedAt: m.startedAt,
    endedAt: isNonNegInt(m.endedAt) ? m.endedAt : undefined,
    changes,
  };
}

/** Fail-quiet: an unparseable health block means "no signal", never an error. */
function parseUpdateHealth(o: unknown): DeviceStatus["updateHealth"] {
  if (typeof o !== "object" || o === null) return undefined;
  const m = o as Record<string, unknown>;
  if (typeof m.packageName !== "string" || m.packageName.length === 0) return undefined;
  if (!isNonNegInt(m.attempts) || !isNonNegInt(m.sinceUnix)) return undefined;
  return {
    packageName: m.packageName,
    versionCode: isNonNegInt(m.versionCode) ? m.versionCode : undefined,
    attempts: m.attempts,
    sinceUnix: m.sinceUnix,
    lastError:
      typeof m.lastError === "string" && m.lastError.length > 0 && m.lastError.length <= 200
        ? m.lastError
        : undefined,
  };
}
