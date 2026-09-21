// Pure mapping: the app's domain Policy → the Kintrinsic device-broker CLAUSE
// payloads `charterd` verifies. This is the heart of remote signing: a parent's
// rule becomes the exact GrantSchedule / GrantBudget on the wire.
//
// The load-bearing subtlety is the `paused` INVERSION (see ./types.ts): the
// app's `paused` means "lift the limit (always allowed)"; the wire's `paused`
// means "block everything". We translate a lifted dimension into an explicit
// no-constraint clause (rather than copying `paused`), so a fresh clause always
// supersedes any previously-cached one with the parent's true current intent.

import { normalizeWebDomain } from "../domain/webDomain";
import type { AlwaysAvailablePolicy, AppsPolicy, Budget, BucketsPolicy, LearningPolicy, Lifeline, ListeningPolicy, Policy, Schedule, ScheduleWindow, Tethering, WebPolicy } from "../domain/types";
import type {
  AppBucket,
  AppRule,
  ClauseKind,
  GrantAlwaysAvailable,
  GrantAppRules,
  GrantLearning,
  ClausePayload,
  GrantApps,
  GrantBudget,
  GrantContent,
  GrantSchedule,
  GrantTethering,
  GrantBuckets,
  GrantListening,
  GrantLifeline,
  GrantUpdate,
  UpdateManifest,
} from "./types";
import { pruneHolds } from "../domain/appHolds";

const HHMM = /^([01]\d|2[0-3]):([0-5]\d)$/;
const MAX_DAILY = 24 * 60;
const MAX_WEEKLY = 7 * 24 * 60;

function assertWindows(windows: ScheduleWindow[]): void {
  for (const w of windows) {
    if (!HHMM.test(w.start) || !HHMM.test(w.end)) {
      throw new Error(`schedule window must be HH:MM, got ${w.start}-${w.end}`);
    }
    if (w.start >= w.end) {
      throw new Error(`schedule window must satisfy start < end, got ${w.start}-${w.end}`);
    }
  }
}

/** Map a domain `Schedule` to the wire `GrantSchedule` (with the paused inversion). */
export function scheduleToGrant(schedule: Schedule, issuedAt: number): GrantSchedule {
  if (!schedule.tz || !schedule.tz.trim()) throw new Error("schedule.tz is required");
  // App-paused = lifted = always allowed → an empty (no-op) weekly, NOT wire-paused.
  if (schedule.paused) {
    return { v: 1, tz: schedule.tz, weekly: {}, issuedAt };
  }
  const weekly: GrantSchedule["weekly"] = {};
  let totalWindows = 0;
  for (const [day, windows] of Object.entries(schedule.weekly)) {
    if (!windows) continue;
    assertWindows(windows);
    weekly[day as keyof GrantSchedule["weekly"]] = windows.map((w) => ({ start: w.start, end: w.end }));
    totalWindows += windows.length;
  }
  if (schedule.overrides) {
    for (const windows of Object.values(schedule.overrides)) {
      assertWindows(windows);
      totalWindows += windows.length;
    }
  }
  // A NON-paused schedule that allows zero time anywhere (every day blocked, no
  // override windows) is the parent's "block all screen time" intent. An all-
  // empty `weekly` would be read by the device as "no schedule = always allowed"
  // (the exact inverse — a silent fail-open), so encode it as an explicit wire
  // `paused: true` (block all). The app's own `paused` means the opposite (lift),
  // and is handled above; this is the only path that emits wire-paused.
  if (totalWindows === 0) {
    return { v: 1, tz: schedule.tz, weekly: {}, paused: true, issuedAt };
  }
  const out: GrantSchedule = { v: 1, tz: schedule.tz, weekly, issuedAt };
  if (schedule.overrides) out.overrides = schedule.overrides;
  return out;
}

/**
 * Map a domain `Budget` to the wire `GrantBudget` (with the paused inversion).
 *
 * `model` is emitted ONLY when it is `"named"` — never `"session"`, and never
 * merely because the field is present but equal to the default. Absent IS the
 * wire's session representation (see `Budget.model`'s own doc and the Rust
 * `TimeModel::of`, which reads exactly the same absence the same way), so a
 * charter authored before this field existed, and one explicitly set back to
 * `"session"`, must serialise to the identical bytes — an old device re-reads
 * the same clause it always did, and no existing golden vector's byte shape
 * moves.
 */
export function budgetToGrant(budget: Budget, issuedAt: number): GrantBudget {
  if (!budget.tz || !budget.tz.trim()) throw new Error("budget.tz is required");
  // App-paused = lifted = no cap → an unconstrained budget, NOT wire-paused (quota 0).
  if (budget.paused) {
    const out: GrantBudget = { v: 1, tz: budget.tz, issuedAt };
    if (budget.model === "named") out.model = "named";
    return out;
  }
  const out: GrantBudget = { v: 1, tz: budget.tz, issuedAt };
  if (budget.model === "named") out.model = "named";
  if (budget.dailyMinutes != null) {
    if (budget.dailyMinutes < 1 || budget.dailyMinutes > MAX_DAILY) {
      throw new Error(`dailyMinutes must be 1..${MAX_DAILY}, got ${budget.dailyMinutes}`);
    }
    out.dailyMinutes = budget.dailyMinutes;
  }
  if (budget.weeklyMinutes != null) {
    if (budget.weeklyMinutes < 1 || budget.weeklyMinutes > MAX_WEEKLY) {
      throw new Error(`weeklyMinutes must be 1..${MAX_WEEKLY}, got ${budget.weeklyMinutes}`);
    }
    out.weeklyMinutes = budget.weeklyMinutes;
  }
  if (budget.weekStart) out.weekStart = budget.weekStart;
  return out;
}

/**
 * Map a domain `WebPolicy` to the wire `GrantContent`. Like schedule/budget, the
 * app's "off" is NOT the wire's `paused`: filtering-off is `revoked` (device
 * reads it as unrestricted), whereas wire `paused` blocks ALL web — so a lifted
 * policy emits `revoked`, and a fresh clause always supersedes a cached one with
 * the parent's true current intent. v1 carries parent-authoritative fields only.
 */
export function contentToGrant(web: WebPolicy, issuedAt: number): GrantContent {
  const g: GrantContent = {
    v: 1,
    posture: web.posture,
    ageTier: web.ageTier,
    issuedAt,
  };
  if (!web.enabled) {
    g.revoked = true; // filtering lifted — unrestricted (never wire-`paused`)
    return g;
  }
  g.safeSearch = web.safeSearch;
  g.youtubeRestrict = web.youtube;
  // Entries saved before the editor vetted them (`https://www.youtube.com/…`,
  // `*.site.com`) are canonicalised here so they start matching; one that
  // can't be read as a domain is sent as it was, never silently dropped.
  const canon = (list: string[]) => [
    ...new Set(
      list
        .map((s) => s.trim())
        .filter(Boolean)
        .map((s) => {
          const r = normalizeWebDomain(s);
          return r.ok ? r.domain : s;
        }),
    ),
  ];
  const allow = canon(web.allow);
  const block = canon(web.block);
  if (allow.length) g.parentAllow = allow;
  if (block.length) g.parentDeny = block;
  return g;
}

/**
 * Map a domain `AppsPolicy` to the wire `GrantApps` (standing per-app policy).
 * The app's "off" is `paused` (lift), so a fresh clause always supersedes a
 * cached one with the parent's current intent. Only the posture-relevant list
 * is carried.
 */
export function appsToGrant(apps: AppsPolicy, issuedAt: number): GrantApps {
  const g: GrantApps = { v: 1, posture: apps.posture, issuedAt };
  // "Remove from device" is emitted BEFORE the paused early-return, on
  // purpose: hiding is not part of the block/allow policy and `paused` is the
  // app's "off" for THAT policy only. A guardian who lifts app blocks for the
  // holidays has not asked for the tablet's forty preinstalled apps back on
  // the home screen — folding the two together would make lifting one rule
  // silently undo a completely different decision. Trimmed and deduped, and
  // dropped entirely (never an empty array) so an unchanged policy keeps a
  // byte-identical clause — the same rule `askFirst` and `holds` follow below.
  const hidden = [...new Set((apps.hidden ?? []).map((s) => s.trim()).filter(Boolean))];
  if (hidden.length) g.hidden = hidden;
  if (!apps.enabled) {
    g.paused = true; // policy lifted — no app is blocked by it
    return g;
  }
  const blocked = apps.blocked.map((s) => s.trim()).filter(Boolean);
  const allowed = apps.allowed.map((s) => s.trim()).filter(Boolean);
  // Computed BEFORE the posture branch (N1, review round 2): under allowlist,
  // an on-request pkg must never ALSO sit in the SIGNED `allowed` list — an
  // always-allowed pkg would defeat "ask first" entirely (the original F1
  // fail-open). This strip happens HERE, on the wire bytes only, never on the
  // stored `AppsPolicy.allowed` itself — `applyAppsFragment` deliberately
  // carries `allowed` through untouched (see its own doc): `prior` there is
  // not always the pre-save draft, it is the ACTUAL saved policy on the next
  // render (`Limits.tsx` signs this function's output, then re-seeds
  // `draftApps`/`savedApps` from it), so stripping there would permanently
  // delete the guardian's own allowlist entry — deleting the on-request group
  // later would have nothing left to restore Minecraft FROM. Only the signed
  // bytes drop it; the guardian's stored allowlist survives a save intact.
  const askFirstSet = new Set((apps.askFirst ?? []).map((s) => s.trim()).filter(Boolean));
  if (apps.posture === "allowlist") {
    const wireAllowed = allowed.filter((p) => !askFirstSet.has(p));
    if (wireAllowed.length) g.allowed = wireAllowed;
  } else if (blocked.length) {
    g.blocked = blocked;
  }
  // askFirst is a pure presentation hint — enforcement never reads it, only
  // `blocked`/`allowed` gate. The invariant it must hold is POSTURE-aware,
  // because "gated" itself is posture-aware:
  //  - blocklist: every askFirst pkg MUST also be in `blocked` (literal), or
  //    the guardian surface would offer an "ask to open" affordance for an
  //    app that isn't gated at all.
  //  - allowlist: "blocked" is DERIVED (not-in-`allowed` = blocked — see
  //    linux `blocked_pkgs_from_apps`/android `app_policy`'s allowlist
  //    branch), so every askFirst pkg must NOT be in `allowed` (found in
  //    review: an allowlist family's on-request app never emitted askFirst at
  //    all, because this code only ever checked `g.blocked`, which an
  //    allowlist clause never sets — the ask affordance was permanently
  //    dead). The strip above already guarantees this by construction (an
  //    askFirst pkg can never survive into `g.allowed`), but the filter below
  //    is the wire's own belt-and-braces check, same as the blocklist branch.
  // Dropped entirely (never an empty array) so an unchanged policy keeps a
  // byte-identical clause.
  if (apps.askFirst?.length) {
    const askFirstTrimmed = apps.askFirst.map((s) => s.trim()).filter(Boolean);
    if (apps.posture === "allowlist") {
      const allowedSet = new Set(g.allowed ?? []);
      const askFirst = askFirstTrimmed.filter((s) => !allowedSet.has(s));
      if (askFirst.length) g.askFirst = askFirst;
    } else if (g.blocked?.length) {
      const blockedSet = new Set(g.blocked);
      const askFirst = askFirstTrimmed.filter((s) => blockedSet.has(s));
      if (askFirst.length) g.askFirst = askFirst;
    }
  }
  // Time-boxed holds, pruned against the clause's OWN issuedAt: a hold already
  // over by the time it is signed is noise the device would drop anyway, and one
  // reaching past the device's cap would be refused outright. Sending only what
  // will be honoured keeps the two sides' reading of the clause identical.
  const holds = pruneHolds(apps.holds, issuedAt);
  if (holds.length) g.holds = holds;
  return g;
}

/**
 * Map a domain `LearningPolicy` to the wire `GrantLearning`. "Off" is
 * `paused` (lift), so a fresh clause always supersedes a cached one with the
 * parent's current intent; app entries are already wire-shaped.
 */
export function learningToGrant(learning: LearningPolicy, issuedAt: number): GrantLearning {
  if (!learning.enabled) {
    // Pausing "free times" pauses the free GRANT. A COSTING site was never
    // getting free time — it lives in this clause only because this clause is
    // where a pinned, resolver-locked window is DEFINED — so dropping it here
    // would silently delete its launcher and, worse, make its window
    // unsanctioned so the site-app lockdown terminates it. A guardian pausing
    // one thing would close a completely different one, with no message
    // anywhere. Free entries still go (unchanged behaviour); costing ones stay.
    const costing = learning.apps.filter((a) => a.free === false);
    return { v: 1, apps: costing, paused: true, issuedAt };
  }
  const g: GrantLearning = { v: 1, apps: learning.apps, issuedAt };
  if (learning.capMinutes != null) g.capMinutes = learning.capMinutes;
  return g;
}

/**
 * Map the lifeline (spec D9) to its wire body. Entries are trimmed and empty
 * rows dropped BEFORE this point (the editor's job); the device re-validates
 * fail-closed either way, so a sloppy entry costs buttons, never safety.
 */
export function lifelineToGrant(lifeline: Lifeline, issuedAt: number): GrantLifeline {
  const g: GrantLifeline = {
    v: 1,
    numbers: lifeline.numbers.map((n) => ({ label: n.label.trim(), number: n.number.trim() })),
    issuedAt,
  };
  // v2 fields are emitted ONLY when set, so a family who touches neither
  // keeps a byte-identical payload (and older devices keep working).
  if (lifeline.emergencyServices) g.emergencyServices = true;
  if (lifeline.torch) g.torch = true;
  // Break-glass is the exception to "only when set": the device reads an
  // ABSENT field as the safety net being UP (`BreakGlassCfg::safety_net`), so
  // "off" has to be SAID — omitting it left a guardian with a switch that
  // changed nothing on the phone. Untouched (undefined) stays absent, which is
  // ON on both sides. Older devices ignore the unknown field either way.
  if (lifeline.breakGlass) {
    g.breakGlass = {
      enabled: lifeline.breakGlass.enabled,
      scope: lifeline.breakGlass.scope,
      durationMinutes: lifeline.breakGlass.durationMinutes,
    };
  }
  return g;
}

export function tetheringToGrant(tethering: Tethering, issuedAt: number): GrantTethering {
  const g: GrantTethering = { v: 1, allow: tethering.allow, issuedAt };
  // Only carry an expiry for an active allowance — a blocked posture is
  // permanent, so an `until` on it would be meaningless (and drift the bytes).
  if (tethering.allow !== "none" && tethering.until != null) g.until = tethering.until;
  return g;
}

/**
 * Map the child's app-scope policies to ONE wire `GrantAppRules` (replace-the-
 * set, mirroring `learning`). Each app-scope Policy becomes an `AppRule` keyed
 * by its `appId` (the pkg the device knows); a per-app allowed-hours schedule
 * reuses `scheduleToGrant`. Device-scope policies are ignored here — they carry
 * their own clauses. Rules with a blank `appId` are dropped (no device key).
 */
/**
 * Map the guardian's bucket set to the wire `GrantBuckets`.
 *
 * Drops incomplete buckets rather than shipping them: the device validates
 * fail-safe (a malformed body caps NOTHING), so a half-filled bucket sent to a
 * device would silently disable every other bucket in the same clause. Better
 * to never put it on the wire.
 */
/**
 * Map the family's listening agreement to the wire.
 *
 * Drops apps that are blank and clamps the grace, because the device fails
 * CLOSED on anything it cannot trust: an out-of-range grace there means "stop",
 * so shipping one would quietly revoke the very permission the guardian just
 * granted. Better to send something valid than something that reads as a
 * setting and behaves as its opposite.
 */
export function listeningToGrant(policy: ListeningPolicy, issuedAt: number): GrantListening {
  const g: GrantListening = {
    v: 1,
    issuedAt,
    mode: policy.mode,
    apps: policy.apps.map((a) => a.trim()).filter(Boolean),
  };
  if (policy.mode === "grace") {
    const m = Math.round(policy.graceMinutes ?? 30);
    g.graceMinutes = Math.min(240, Math.max(1, m));
  }
  return g;
}

/**
 * Map the family's always-available list to the wire.
 *
 * Entries already past their expiry at signing time are dropped: they would be
 * inert on the device anyway, and leaving them on the wire makes the guardian's
 * own summary read as though a lapsed grant were live.
 */
export function alwaysAvailableToGrant(
  policy: AlwaysAvailablePolicy,
  issuedAt: number,
): GrantAlwaysAvailable {
  return {
    v: 1,
    issuedAt,
    apps: policy.apps
      .filter((a) => a.untilUnix === undefined || a.untilUnix > issuedAt)
      .map((a) =>
        a.untilUnix === undefined ? { pkg: a.pkg } : { pkg: a.pkg, untilUnix: a.untilUnix },
      ),
  };
}

const MAX_BUCKET_DAILY = 1440;
const MAX_BUCKET_WEEKLY = 10080;

/** Fail-safe filter mirroring the device's `GrantBuckets::is_valid`: at least
 *  one of daily/weekly must be present and each axis within its own bound. */
function validBucket(b: AppBucket): boolean {
  if (!/^[a-z0-9-]{1,40}$/.test(b.id)) return false;
  if (b.label.length === 0 || b.label.length > 32) return false;
  if (b.apps.length === 0) return false;
  if (b.dailyMinutes == null && b.weeklyMinutes == null) return false;
  if (b.dailyMinutes != null && (b.dailyMinutes < 1 || b.dailyMinutes > MAX_BUCKET_DAILY)) {
    return false;
  }
  if (b.weeklyMinutes != null && (b.weeklyMinutes < 1 || b.weeklyMinutes > MAX_BUCKET_WEEKLY)) {
    return false;
  }
  return true;
}

/**
 * THE VERSION RULE (binding, byte-pinned both directions — see
 * `bucketsClause.test.ts` and the Rust `wire_agreement` twin in
 * `charter-schedule::buckets`): emit `v: 1` iff EVERY bucket has
 * `dailyMinutes`; `v: 2` the moment ANY bucket is weekly-only. An empty set
 * stays `v: 1` (vacuously "every bucket has dailyMinutes"), which is exactly
 * what a family who never touched buckets already sent.
 */
export function bucketsToGrant(policy: BucketsPolicy, issuedAt: number): GrantBuckets {
  const buckets: AppBucket[] = policy.buckets
    .map((b) => {
      const out: AppBucket = {
        id: b.id,
        label: b.label.trim(),
        apps: b.apps.map((a) => a.trim()).filter(Boolean),
      };
      if (b.dailyMinutes != null) out.dailyMinutes = b.dailyMinutes;
      if (b.weeklyMinutes != null) out.weeklyMinutes = b.weeklyMinutes;
      return out;
    })
    .filter(validBucket);
  const v: 1 | 2 = buckets.every((b) => b.dailyMinutes != null) ? 1 : 2;
  const g: GrantBuckets = { v, buckets, tz: policy.tz, issuedAt };
  if (policy.weekStart) g.weekStart = policy.weekStart;
  if (!policy.enabled) g.paused = true;
  return g;
}

export function appRulesToGrant(policies: Policy[], issuedAt: number): GrantAppRules {
  const rules: AppRule[] = [];
  for (const p of policies) {
    if (p.scope.kind !== "app") continue;
    const pkg = p.scope.appId.trim();
    if (!pkg) continue;
    const rule: AppRule = { pkg, blocked: p.blocked ?? false };
    if (p.scope.label) rule.label = p.scope.label;
    if (p.schedule) rule.schedule = scheduleToGrant(p.schedule, issuedAt);
    rules.push(rule);
  }
  return { v: 1, rules, issuedAt };
}

/**
 * Produce the CLAUSE payload(s) for a child from one policy. Only **device-scope**
 * policies carry clauses; an app-scope policy is the per-app dimension (not a
 * CLAUSE) and yields none. `subject` is the child's dependant pubkey hex
 * (omit/null = the single-child default).
 */
/**
 * Build the `update` clause body from the site's artifact manifest (#44).
 * `url` is passed by the caller (same-origin absolute URL) so the manifest
 * stays a pure artifact description.
 */
export function updateToGrant(manifest: UpdateManifest, url: string): GrantUpdate {
  return {
    v: 1,
    packageName: "org.forgesworn.charter",
    versionCode: manifest.versionCode,
    versionName: manifest.versionName,
    url,
    apkSha256: manifest.apkSha256,
    signerCertSha256: manifest.certSha256,
  };
}

export function policyToClauses(
  subject: string | null,
  policy: Policy,
  issuedAt: number,
): ClausePayload[] {
  if (policy.scope.kind !== "device") return [];
  const out: ClausePayload[] = [];
  const base = (
    kind: ClauseKind,
    body:
      | GrantSchedule
      | GrantBudget
      | GrantContent
      | GrantApps
      | GrantLearning
      | GrantTethering
      | GrantLifeline
      | GrantBuckets
      | GrantListening
      | GrantAlwaysAvailable,
  ): ClausePayload => {
    const c: ClausePayload = { v: 1, kind, issuedAt, body };
    if (subject) c.subject = subject;
    return c;
  };
  if (policy.schedule) out.push(base("schedule", scheduleToGrant(policy.schedule, issuedAt)));
  if (policy.budget) out.push(base("budget", budgetToGrant(policy.budget, issuedAt)));
  if (policy.web) out.push(base("content", contentToGrant(policy.web, issuedAt)));
  if (policy.apps) out.push(base("apps", appsToGrant(policy.apps, issuedAt)));
  if (policy.learning)
    out.push(base("learning", learningToGrant(policy.learning, issuedAt)));
  if (policy.tethering)
    out.push(base("tethering", tetheringToGrant(policy.tethering, issuedAt)));
  if (policy.lifeline && policy.lifeline.numbers.length > 0)
    out.push(base("lifeline", lifelineToGrant(policy.lifeline, issuedAt)));
  if (policy.buckets) out.push(base("buckets", bucketsToGrant(policy.buckets, issuedAt)));
  if (policy.listening)
    out.push(base("listening", listeningToGrant(policy.listening, issuedAt)));
  if (policy.alwaysAvailable)
    out.push(base("alwaysavailable", alwaysAvailableToGrant(policy.alwaysAvailable, issuedAt)));
  return out;
}
