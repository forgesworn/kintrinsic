// Pure mapping: the parent's approve/deny on a `time.extend` ask → the exact
// GRANT payload `charterd` verifies (`spec/contract.md` "Device brokering —
// time.extend"). The load-bearing bits: `reqId` + `nonce` are echoed VERBATIM
// (the device's request binding), `limitHit` is echoed so the params-echo
// check is exact, minutes are clamped onto the wire's u16 0..=1440, and `exp`
// is the natural end-of-day in the resolved tz (`pickExtendTz`) CLAMPED to
// the device's own validity cap (`deviceExtendCapEod` — schedule tz, else
// budget tz, else UTC; the buckets clause's tz is never part of the
// device's cap) — the extension is today-only and the device rejects an
// expiry beyond its own end-of-day (+300s skew), silently from this app's
// point of view, so the clamp is load-bearing, not cosmetic.

import type {
  ApkSource,
  AppOpenGrantParams,
  GrantPayload,
  InstallApkGrantParams,
  TimeExtendGrantParams,
  TimeExtendLimitHit,
} from "./types";

/** A GrantPayload statically known to carry time.extend params. */
export type TimeExtendGrantPayload = GrantPayload & {
  op: "time.extend";
  params: TimeExtendGrantParams;
};

export const MAX_EXTEND_MINUTES = 1440;

/** Clamp granted minutes onto the wire's u16 range (0..=1440), flooring fractions. */
export function clampGrantMinutes(minutes: number): number {
  if (!Number.isFinite(minutes)) return 0;
  return Math.min(MAX_EXTEND_MINUTES, Math.max(0, Math.floor(minutes)));
}

/**
 * The tz `exp` (end-of-day) must be computed in, chosen by the dimension the
 * lock actually hit. Prefer the locked dimension's tz, fall back to the
 * other WHOLE-DEVICE dimension, then — only when NEITHER schedule nor budget
 * exists at all — the buckets clause's own tz; `undefined` only when the app
 * knows none of the three (state divergence — the caller must refuse rather
 * than emit a phone-local, silently-doomed grant).
 *
 * `bucketsTz` is a FALLBACK, never an override (review round 2, hardware
 * round 2026-08-03): a working schedule/budget ward must keep signing
 * EXACTLY what it signs today. Earlier this round preferred `bucketsTz`
 * outright on a `bucket` hit — but `buckets.tz` is seeded from the
 * guardian's OWN local tz at authoring time, not derived from the
 * schedule/budget the way this whole function assumes, so a schedule+buckets
 * ward with divergent tz's could sign an `exp` the device's actual cap
 * (`deviceExtendCapEod` below — schedule/budget only, buckets never
 * consulted there) would reject. Buckets-only wards (no schedule, no
 * budget — C-1's own minimal configuration) are the ONLY case this
 * fallback fires for, and `buildTimeExtendGrant` additionally clamps `exp`
 * to the device's real cap regardless of which tz was picked here, so even
 * a wildly divergent `bucketsTz` degrades to a same-day-shortened grant,
 * never a silently-rejected one.
 */
export function pickExtendTz(
  limitHit: TimeExtendLimitHit,
  scheduleTz?: string,
  budgetTz?: string,
  bucketsTz?: string,
): string | undefined {
  if (limitHit === "bucket") return scheduleTz ?? budgetTz ?? bucketsTz;
  return limitHit === "budget"
    ? (budgetTz ?? scheduleTz)
    : (scheduleTz ?? budgetTz);
}

/**
 * Mirrors the device's OWN `time.extend` validity cap EXACTLY
 * (`charter-spine::enforcer_runtime::time_extend_eod`, read by the enactor
 * as `ctx.eod_unix` and enforced as a hard `Terminal` rejection beyond
 * `+300s` skew): the LATER of the schedule- and budget-tz local midnights,
 * over whichever of those two clauses actually exist; UTC midnight when
 * NEITHER does. Deliberately does **not** consult the buckets clause's tz —
 * the device doesn't either, today. `buildTimeExtendGrant` clamps `exp` to
 * this so a grant this app signs is never one the device silently drops.
 *
 * (A future ward release may teach the device to consult the buckets tz for
 * this cap too, matching how it already keys the bucket's own pool rollover
 * by it — see `spec/contract.md`'s "Device brokering — time.extend". Once
 * that ships, this clamp can relax to include it; until then, widening the
 * clamp would just reintroduce the silent-rejection bug.)
 */
export function deviceExtendCapEod(ts: number, scheduleTz?: string, budgetTz?: string): number {
  if (scheduleTz && budgetTz) {
    return Math.max(endOfDayUnix(ts, scheduleTz), endOfDayUnix(ts, budgetTz));
  }
  if (scheduleTz) return endOfDayUnix(ts, scheduleTz);
  if (budgetTz) return endOfDayUnix(ts, budgetTz);
  return endOfDayUnix(ts, "UTC");
}

/**
 * The `dayKey` a device reporting in `tz` would stamp on a STATUS at
 * `nowSecs` — the guardian-side mirror of the device's own day boundary, for
 * any caller that needs to tell "still today" from "before the ward's
 * midnight" (e.g. `domain/unrecognisedTime.ts`'s day guard: a STATUS from
 * just before midnight must not go on reading as "today" once the ward's day
 * has actually turned, even if the device hasn't reported again yet). Same
 * unknown-tz fallback as `endOfDayUnix`/`startOfDayUnix`.
 */
export function currentDayKey(nowSecs: number, tz?: string): string {
  try {
    return tzDayParts(nowSecs, tz).dayKey;
  } catch {
    return tzDayParts(nowSecs).dayKey;
  }
}

/** The tz-local calendar day + seconds-into-day at `atSecs`. Throws on a tz the
 *  runtime doesn't know (caller falls back to the local tz). */
function tzDayParts(atSecs: number, tz?: string): { dayKey: string; secsIntoDay: number } {
  const fmt = new Intl.DateTimeFormat("en-CA", {
    timeZone: tz, // undefined = the runtime's local tz
    hourCycle: "h23",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
  const p: Record<string, string> = {};
  for (const part of fmt.formatToParts(new Date(atSecs * 1000))) p[part.type] = part.value;
  return {
    dayKey: `${p.year}-${p.month}-${p.day}`,
    secsIntoDay: Number(p.hour) * 3600 + Number(p.minute) * 60 + Number(p.second),
  };
}

/**
 * Unix seconds of the NEXT midnight in `tz` after `nowSecs` — "end of today"
 * in the child's schedule tz. An unknown/absent tz falls back to the phone's
 * local midnight. Starts from the naive 24h estimate and nudges onto the real
 * tz-midnight, so DST-shifted (23h/25h, even 24.5h) days land exactly.
 */
export function endOfDayUnix(nowSecs: number, tz?: string): number {
  let now: { dayKey: string; secsIntoDay: number };
  try {
    now = tzDayParts(nowSecs, tz);
  } catch {
    tz = undefined; // unknown tz → local-midnight fallback
    now = tzDayParts(nowSecs);
  }
  let eod = nowSecs + (86_400 - now.secsIntoDay);
  for (let i = 0; i < 4; i += 1) {
    const at = tzDayParts(eod, tz);
    if (at.dayKey !== now.dayKey && at.secsIntoDay === 0) break;
    // Still today (a 25h day fell short) → push to that day's naive midnight;
    // past midnight (a 23h day overshot) → pull back onto the boundary.
    eod += at.dayKey === now.dayKey ? 86_400 - at.secsIntoDay : -at.secsIntoDay;
  }
  return eod;
}

/**
 * Unix seconds of the START of `tz`'s calendar day containing `nowSecs` — the
 * mirror of `endOfDayUnix`, reusing the same tz-aware day parts so the two
 * always agree on where the boundary falls (DST included). Used to scope
 * "granted today" reflections (`domain/groupProgress.ts`'s extras join) to
 * the CHILD's day, never the guardian phone's.
 */
export function startOfDayUnix(nowSecs: number, tz?: string): number {
  let now: { dayKey: string; secsIntoDay: number };
  try {
    now = tzDayParts(nowSecs, tz);
  } catch {
    now = tzDayParts(nowSecs); // unknown tz → local-midnight fallback
  }
  return nowSecs - now.secsIntoDay;
}

/** The parent's decision on one `time.extend` REQUEST, ready to become a GRANT. */
export interface TimeExtendDecision {
  /** Echoed verbatim from the REQUEST (32-byte hex). */
  reqId: string;
  /** Echoed verbatim from the REQUEST (32-byte hex). */
  nonce: string;
  decision: "allow" | "deny";
  /** Minutes the parent granted (allow only — a deny always carries 0). */
  minutesGranted: number;
  /** Echoed verbatim from the REQUEST. */
  limitHit: TimeExtendLimitHit;
  /** Echoed verbatim from the REQUEST when `limitHit === 'bucket'` — the
   *  specific named-times group the extension credits. Absent for the
   *  whole-device dimensions, exactly like the REQUEST side. */
  bucketId?: string;
  /** Unix seconds now — becomes the grant `ts`. */
  ts: number;
  /** The tz the grant's "natural" end-of-day is computed in (`pickExtendTz`'s
   *  resolved dimension). Absent/unknown = the phone's local midnight. */
  tz?: string;
  /** The child's ACTUAL schedule/budget clause tz's — separate from `tz`
   *  above — used ONLY to compute `deviceExtendCapEod`'s clamp. Passing
   *  these even when `tz` came from neither (the buckets-only fallback) is
   *  what keeps a divergent-tz grant from being silently rejected by the
   *  device's own cap (review round 2, hardware round 2026-08-03). */
  scheduleTz?: string;
  budgetTz?: string;
}

/** Build the GRANT payload for a `time.extend` decision. */
export function buildTimeExtendGrant(d: TimeExtendDecision): TimeExtendGrantPayload {
  // Contract: exp > ts (the device rejects a degenerate expiry), AND exp must
  // not exceed the device's OWN validity cap (`deviceExtendCapEod`) or the
  // enactor rejects the whole grant as Terminal — silently, from this app's
  // point of view (it publishes, sees no error, and the device just never
  // enacts it). The natural end-of-day (in `d.tz`, whichever dimension that
  // resolved to) is clamped DOWN to the device's cap when the two diverge;
  // never up — widening `exp` past what was asked is never correct. Only the
  // VALIDITY WINDOW shortens here; crediting is independent of `exp`
  // (`ExtensionLedger::apply_bucket` never reads it), and the device rolls
  // the extension pool on the same enforcement (schedule-first, UTC-fallback)
  // tz this cap mirrors — for a buckets-only ward the clamp target IS the
  // pool's own roll boundary. So the clamp can never under-credit, only
  // (rarely) shorten how long a grant stays enactable if delivery is delayed.
  const natural = endOfDayUnix(d.ts, d.tz);
  const cap = deviceExtendCapEod(d.ts, d.scheduleTz, d.budgetTz);
  const exp = Math.max(Math.min(natural, cap), d.ts + 1);
  const params: TimeExtendGrantParams = {
    minutesGranted: d.decision === "allow" ? clampGrantMinutes(d.minutesGranted) : 0,
    limitHit: d.limitHit,
  };
  // Echoed regardless of allow/deny — exactly like limitHit itself.
  if (d.bucketId !== undefined) params.bucketId = d.bucketId;
  return {
    v: 1,
    op: "time.extend",
    reqId: d.reqId,
    nonce: d.nonce,
    decision: d.decision,
    ts: d.ts,
    exp,
    params,
  };
}

// ---------------------------------------------------------------------------
// install.apk — the parent's per-app, single-use install approval → GRANT.
// ---------------------------------------------------------------------------

/** A GrantPayload statically known to carry install.apk params. */
export type InstallApkGrantPayload = GrantPayload & {
  op: "install.apk";
  params: InstallApkGrantParams;
};

/** How long an install approval stays enactable (seconds). Unlike time.extend
 *  there is no schedule tz to key an end-of-day off, so the window is a fixed
 *  short-lived one — long enough for the phone's next poll + install, short
 *  enough that a stale approval doesn't linger (mirrors the device's own
 *  RELEASE_FRESHNESS margin sensibility, and well inside the 7-day queue sweep). */
export const INSTALL_GRANT_TTL_SECS = 24 * 60 * 60;

/** All-zero digest carried by a DENY (a refusal pins no provenance). The device
 *  never reads a cert on a deny — `verify_grant` only parses params for an
 *  allow — so this is the install analog of a `time.extend` deny's 0 minutes. */
export const DENY_CERT_SENTINEL = "0".repeat(64);

/** The parent's decision on one `install.apk` REQUEST, ready to become a GRANT.
 *  `signerCertSha256` is the TRUSTED pin from the curated catalog — never from
 *  the phone's request — and is REQUIRED to allow; a deny needs none. */
export interface InstallApkDecision {
  /** Echoed verbatim from the REQUEST (32-byte hex). */
  reqId: string;
  /** Echoed verbatim from the REQUEST (32-byte hex). */
  nonce: string;
  decision: "allow" | "deny";
  packageName: string;
  /** Lowercase-hex SHA-256 of the vetted signing cert. Required for an allow;
   *  omit for a deny. */
  signerCertSha256?: string;
  /** Optional minimum version the device will accept. */
  versionCode?: number;
  source: ApkSource;
  /** Unix seconds now — becomes the grant `ts`. */
  ts: number;
}

/** Build the GRANT payload for an `install.apk` decision. On an ALLOW the device
 *  re-derives the staged archive's signer digest and refuses to commit unless it
 *  equals `signerCertSha256` (signing continuity, BEFORE the installer runs) — so
 *  an allow WITHOUT a pinned cert is a programming error and throws. A DENY needs
 *  no cert (the device never checks one), so it carries the zero sentinel. */
export function buildInstallApkGrant(d: InstallApkDecision): InstallApkGrantPayload {
  if (d.decision === "allow" && !d.signerCertSha256) {
    throw new Error("install.apk allow requires a pinned signerCertSha256");
  }
  const params: InstallApkGrantParams = {
    packageName: d.packageName,
    signerCertSha256: d.decision === "allow" ? d.signerCertSha256! : DENY_CERT_SENTINEL,
    source: d.source,
  };
  if (d.versionCode !== undefined) params.versionCode = d.versionCode;
  return {
    v: 1,
    op: "install.apk",
    reqId: d.reqId,
    nonce: d.nonce,
    decision: d.decision,
    ts: d.ts,
    exp: d.ts + INSTALL_GRANT_TTL_SECS,
    params,
  };
}

/**
 * The warning a ward is owed before a stand-down locks her, and what Kintrinsic
 * sends when the guardian names none. Mirrors the Rust
 * `DEFAULT_STANDDOWN_GRACE_SECS`; the device clamps to 1..=600 regardless, so
 * this is a default rather than a promise.
 */
export const DEFAULT_STANDDOWN_GRACE_SECS = 60;

// ---------------------------------------------------------------------------
// app.open — the parent's one-tap answer to "can I open Minecraft?".
// ---------------------------------------------------------------------------

/** A GrantPayload statically known to carry (empty) app.open params. */
export type AppOpenGrantPayload = GrantPayload & { op: "app.open"; params: AppOpenGrantParams };

/**
 * How long an `app.open` GRANT stays enactable. There is no schedule tz to
 * key an end-of-day off (and unlike `time.extend` the grant carries no
 * enactable params at all — see `AppOpenGrantParams`), so this mirrors
 * `INSTALL_GRANT_TTL_SECS`: long enough for the ward's next poll.
 */
export const APP_OPEN_GRANT_TTL_SECS = 24 * 60 * 60;

/** The parent's decision on one `app.open` REQUEST, ready to become a GRANT. */
export interface AppOpenDecision {
  /** Echoed verbatim from the REQUEST (32-byte hex). */
  reqId: string;
  /** Echoed verbatim from the REQUEST (32-byte hex). */
  nonce: string;
  decision: "allow" | "deny";
  /** Echoed verbatim from the REQUEST — the params-echo check, same as every
   *  other op. */
  pkg: string;
  /** Minutes the parent granted (allow only — a deny always carries 0,
   *  clamped exactly like `time.extend`'s `minutesGranted`). */
  minutesGranted: number;
  /** Unix seconds now — becomes the grant `ts`. */
  ts: number;
}

/**
 * Build the GRANT payload for an `app.open` decision — the "plain Decision"
 * half of the answer (`spec/contract.md` "App-open ask"), shaped exactly like
 * `time.extend`'s (fixed 2026-08-03, review round 1 — see `AppOpenGrantParams`
 * for why the original empty-object shape silently deadlocked every allow).
 * It carries no ENACTABLE effect of its own; the app itself is actually let
 * through by a SEPARATE re-signed `apps` clause carrying an `AppHold` (see
 * `domain/appHolds.ts`), built alongside this by the caller. A `deny` needs
 * no clause change at all.
 */
export function buildAppOpenGrant(d: AppOpenDecision): AppOpenGrantPayload {
  return {
    v: 1,
    op: "app.open",
    reqId: d.reqId,
    nonce: d.nonce,
    decision: d.decision,
    ts: d.ts,
    exp: d.ts + APP_OPEN_GRANT_TTL_SECS,
    params: {
      pkg: d.pkg,
      minutesGranted: d.decision === "allow" ? clampGrantMinutes(d.minutesGranted) : 0,
    },
  };
}

/** The one-tap windows an `app.open` allow offers. */
export type AppOpenWindow = "30m" | "1h" | "restOfDay";

/**
 * The absolute instant one of the one-tap `app.open` windows resolves to.
 * `30m`/`1h` are plain durations off `nowUnix`; `restOfDay` needs the CHILD's
 * own schedule/budget tz (the same guardian-clock trap `endOfDayUnix` exists
 * to avoid) and returns `null` — never a silent guardian-local fallback —
 * when it's unknown, so the picker can hide the button instead of quietly
 * overshooting the device's own day roll.
 */
export function appOpenWindowUnix(
  window: AppOpenWindow,
  nowUnix: number,
  childTz?: string,
): number | null {
  if (window === "30m") return nowUnix + 30 * 60;
  if (window === "1h") return nowUnix + 60 * 60;
  if (!childTz) return null;
  return endOfDayUnix(nowUnix, childTz);
}
