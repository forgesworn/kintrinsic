// The reverse of ./clause.ts: a received CLAUSE body back into what the
// rule screens edit (decision 3, seeding). Another guardian's phone shares
// the rules in force in the family roster; this is how they reach this
// phone's own screens.
//
// Lossless or lossy is decided per clause, not per kind, by one honest test:
// map the body back, map the result forward again with the clause's own
// `issuedAt`, and compare. Only a body that comes back byte for byte (as
// canonical JSON) is shown on a screen; any other one (a field this app never
// writes, a shape from a newer app, the wire's `paused`/`revoked` where the
// app would never send them) is lossy, and is kept and shown as set
// elsewhere instead of being bent into something the screen can hold.
//
// `prior` supplies what the wire never carries and the screen keeps anyway
// (the inactive app list, a lifted website filter's lists, Free-time apps
// while learning is paused), so seeding a rule never wipes those.

import type {
  AlwaysAvailablePolicy,
  AppsPolicy,
  Budget,
  BucketsPolicy,
  Connections,
  LearningAppSel,
  LearningPolicy,
  Lifeline,
  ListeningPolicy,
  NamedTime,
  Policy,
  Schedule,
  Tethering,
  WebPolicy,
} from "../domain/types";
import {
  alwaysAvailableToGrant,
  appRulesToGrant,
  appsToGrant,
  budgetToGrant,
  bucketsToGrant,
  connectionsToGrant,
  contentToGrant,
  debuggingToGrant,
  learningToGrant,
  lifelineToGrant,
  listeningToGrant,
  scheduleToGrant,
  tetheringToGrant,
  timesToGrant,
} from "./clause";
import type {
  GrantAlwaysAvailable,
  GrantAppRules,
  GrantApps,
  GrantBudget,
  GrantBuckets,
  GrantConnections,
  GrantContent,
  GrantDebugging,
  GrantLearning,
  GrantLifeline,
  GrantListening,
  GrantSchedule,
  GrantTethering,
  GrantTimes,
} from "./types";

/** What a received clause becomes on this phone's screens. */
export type ReadRule =
  | { kind: "schedule"; value: Schedule }
  | { kind: "budget"; value: Budget }
  | { kind: "content"; value: WebPolicy }
  | { kind: "apps"; value: AppsPolicy }
  | { kind: "learning"; value: LearningPolicy }
  | { kind: "tethering"; value: Tethering }
  | { kind: "connections"; value: Connections }
  | { kind: "lifeline"; value: Lifeline }
  | { kind: "buckets"; value: BucketsPolicy }
  | { kind: "listening"; value: ListeningPolicy }
  | { kind: "alwaysavailable"; value: AlwaysAvailablePolicy }
  | { kind: "times"; value: NamedTime[] }
  | { kind: "apprules"; value: AppRuleRead[] }
  | { kind: "debugging"; value: { allow: boolean; until?: number } };

/** One singled-out app from an `apprules` clause (an app-scope policy without its id). */
export interface AppRuleRead {
  appId: string;
  label: string;
  blocked: boolean;
  schedule?: Schedule;
}

/** The clause kinds a screen on this phone edits. */
export const SCREEN_KINDS = [
  "schedule",
  "budget",
  "content",
  "apps",
  "learning",
  "tethering",
  "connections",
  "lifeline",
  "buckets",
  "listening",
  "alwaysavailable",
  "times",
  "apprules",
  "debugging",
] as const;

/** What the screen keeps that the wire may not carry. */
export interface ReadPrior {
  policy?: Policy;
}

/** JSON with object keys sorted, so two bodies compare by content, not key order. */
export function canonicalJson(x: unknown): string {
  if (Array.isArray(x)) return `[${x.map(canonicalJson).join(",")}]`;
  if (x !== null && typeof x === "object") {
    const o = x as Record<string, unknown>;
    return `{${Object.keys(o)
      .filter((k) => o[k] !== undefined)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonicalJson(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(x);
}

const isObj = (x: unknown): x is Record<string, unknown> => typeof x === "object" && x !== null && !Array.isArray(x);
const strings = (x: unknown): string[] => (Array.isArray(x) ? x.filter((s): s is string => typeof s === "string") : []);

function scheduleBack(g: GrantSchedule): Schedule {
  const weekly = isObj(g.weekly) ? (g.weekly as Schedule["weekly"]) : {};
  const windows = Object.values(weekly).reduce((n, w) => n + (Array.isArray(w) ? w.length : 0), 0);
  const overrides = isObj(g.overrides) ? g.overrides : undefined;
  if (g.paused) return { tz: g.tz, weekly: {} };
  // An empty weekly with no overrides is the app's "lift" (always allowed).
  if (windows === 0 && !overrides) return { tz: g.tz, weekly: {}, paused: true };
  const out: Schedule = { tz: g.tz, weekly };
  if (overrides) out.overrides = overrides;
  return out;
}

function budgetBack(g: GrantBudget): Budget {
  const out: Budget = { tz: g.tz };
  if (g.model === "named") out.model = "named";
  if (g.dailyMinutes != null) out.dailyMinutes = g.dailyMinutes;
  if (g.weeklyMinutes != null) out.weeklyMinutes = g.weeklyMinutes;
  if (g.weekStart) out.weekStart = g.weekStart;
  // No cap at all reads the same either way on the device; the app calls it lifted.
  if (out.dailyMinutes == null && out.weeklyMinutes == null && !out.weekStart) out.paused = true;
  return out;
}

function contentBack(g: GrantContent, prior?: WebPolicy): WebPolicy {
  if (g.revoked) {
    return {
      enabled: false,
      posture: g.posture,
      ageTier: g.ageTier,
      allow: prior?.allow ?? [],
      block: prior?.block ?? [],
      youtube: prior?.youtube ?? "off",
      safeSearch: prior?.safeSearch ?? false,
    };
  }
  return {
    enabled: true,
    posture: g.posture,
    ageTier: g.ageTier,
    allow: strings(g.parentAllow),
    block: strings(g.parentDeny),
    youtube: (g.youtubeRestrict ?? "off") as WebPolicy["youtube"],
    safeSearch: g.safeSearch === true,
  };
}

function appsBack(g: GrantApps, prior?: AppsPolicy): AppsPolicy {
  const out: AppsPolicy = {
    enabled: !g.paused,
    posture: g.posture,
    blocked: prior?.blocked ?? [],
    allowed: prior?.allowed ?? [],
  };
  const hidden = strings(g.hidden);
  if (hidden.length) out.hidden = hidden;
  if (g.paused) {
    if (prior?.holds) out.holds = prior.holds;
    if (prior?.askFirst) out.askFirst = prior.askFirst;
    return out;
  }
  const askFirst = strings(g.askFirst);
  if (g.posture === "allowlist") {
    // The stored allowlist keeps "ask first" apps; only the signed bytes strip them.
    out.allowed = [...new Set([...strings(g.allowed), ...askFirst])];
  } else {
    out.blocked = strings(g.blocked);
  }
  if (askFirst.length) out.askFirst = askFirst;
  if (Array.isArray(g.holds) && g.holds.length) out.holds = g.holds;
  return out;
}

function learningBack(g: GrantLearning, prior?: LearningPolicy): LearningPolicy {
  const apps = (Array.isArray(g.apps) ? g.apps : []) as LearningAppSel[];
  if (g.paused) {
    // Paused carries only the costing sites; the Free-time apps stay as they were here.
    const ids = new Set(apps.map((a) => a.id));
    const free = (prior?.apps ?? []).filter((a) => a.free !== false && !ids.has(a.id));
    const out: LearningPolicy = { enabled: false, apps: [...apps, ...free] };
    if (prior?.capMinutes != null) out.capMinutes = prior.capMinutes;
    return out;
  }
  const out: LearningPolicy = { enabled: true, apps };
  if (g.capMinutes != null) out.capMinutes = g.capMinutes;
  return out;
}

function lifelineBack(g: GrantLifeline): Lifeline {
  const out: Lifeline = { numbers: Array.isArray(g.numbers) ? g.numbers.map((n) => ({ label: n.label, number: n.number })) : [] };
  if (g.emergencyServices) out.emergencyServices = true;
  if (g.torch) out.torch = true;
  if (g.breakGlass) out.breakGlass = { ...g.breakGlass };
  return out;
}

function bucketsBack(g: GrantBuckets): BucketsPolicy {
  const out: BucketsPolicy = {
    enabled: !g.paused,
    tz: g.tz,
    buckets: (Array.isArray(g.buckets) ? g.buckets : []).map((b) => {
      const r: BucketsPolicy["buckets"][number] = { id: b.id, label: b.label, apps: strings(b.apps) };
      if (b.dailyMinutes != null) r.dailyMinutes = b.dailyMinutes;
      if (b.weeklyMinutes != null) r.weeklyMinutes = b.weeklyMinutes;
      return r;
    }),
  };
  if (g.weekStart) out.weekStart = g.weekStart;
  return out;
}

function listeningBack(g: GrantListening): ListeningPolicy {
  const out: ListeningPolicy = { mode: g.mode, apps: strings(g.apps) };
  if (g.graceMinutes != null) out.graceMinutes = g.graceMinutes;
  return out;
}

function appRulesBack(g: GrantAppRules): AppRuleRead[] {
  return (Array.isArray(g.rules) ? g.rules : []).map((r) => {
    const out: AppRuleRead = { appId: r.pkg, label: r.label ?? "", blocked: r.blocked === true };
    if (r.schedule) out.schedule = scheduleBack(r.schedule);
    return out;
  });
}

/** Map back without checking; throws on a body too malformed to read. */
function back(kind: string, body: Record<string, unknown>, prior: ReadPrior): ReadRule | null {
  const p = prior.policy;
  switch (kind) {
    case "schedule":
      return { kind, value: scheduleBack(body as unknown as GrantSchedule) };
    case "budget":
      return { kind, value: budgetBack(body as unknown as GrantBudget) };
    case "content":
      return { kind, value: contentBack(body as unknown as GrantContent, p?.web) };
    case "apps":
      return { kind, value: appsBack(body as unknown as GrantApps, p?.apps) };
    case "learning":
      return { kind, value: learningBack(body as unknown as GrantLearning, p?.learning) };
    case "tethering": {
      const g = body as unknown as GrantTethering;
      return { kind, value: g.until != null ? { allow: g.allow, until: g.until } : { allow: g.allow } };
    }
    case "connections": {
      const g = body as unknown as GrantConnections;
      return { kind, value: { wifi: g.wifi, bluetooth: g.bluetooth } };
    }
    case "lifeline":
      return { kind, value: lifelineBack(body as unknown as GrantLifeline) };
    case "buckets":
      return { kind, value: bucketsBack(body as unknown as GrantBuckets) };
    case "listening":
      return { kind, value: listeningBack(body as unknown as GrantListening) };
    case "alwaysavailable": {
      const g = body as unknown as GrantAlwaysAvailable;
      return { kind, value: { apps: Array.isArray(g.apps) ? g.apps.map((a) => ({ ...a })) : [] } };
    }
    case "times": {
      const g = body as unknown as GrantTimes;
      return { kind, value: Array.isArray(g.times) ? g.times : [] };
    }
    case "apprules":
      return { kind, value: appRulesBack(body as unknown as GrantAppRules) };
    case "debugging": {
      const g = body as unknown as GrantDebugging;
      return { kind, value: g.until != null ? { allow: g.allow, until: g.until } : { allow: g.allow } };
    }
    default:
      return null;
  }
}

/** Map forward again, exactly as this app would sign it. */
function forward(r: ReadRule, issuedAt: number): unknown {
  switch (r.kind) {
    case "schedule":
      return scheduleToGrant(r.value, issuedAt);
    case "budget":
      return budgetToGrant(r.value, issuedAt);
    case "content":
      return contentToGrant(r.value, issuedAt);
    case "apps":
      return appsToGrant(r.value, issuedAt);
    case "learning":
      return learningToGrant(r.value, issuedAt);
    case "tethering":
      return tetheringToGrant(r.value, issuedAt);
    case "connections":
      return connectionsToGrant(r.value, issuedAt);
    case "lifeline":
      return lifelineToGrant(r.value, issuedAt);
    case "buckets":
      return bucketsToGrant(r.value, issuedAt);
    case "listening":
      return listeningToGrant(r.value, issuedAt);
    case "alwaysavailable":
      return alwaysAvailableToGrant(r.value, issuedAt);
    case "times":
      return timesToGrant(r.value, issuedAt);
    case "apprules":
      return appRulesToGrant(
        r.value.map((a, i) => ({
          id: String(i),
          scope: { kind: "app" as const, appId: a.appId, label: a.label },
          blocked: a.blocked,
          ...(a.schedule ? { schedule: a.schedule } : {}),
        })),
        issuedAt,
      );
    case "debugging":
      return debuggingToGrant(r.value, issuedAt);
  }
}

/**
 * A received clause payload (`{ v, kind, issuedAt, body }`) as a screen would
 * hold it, or `null` when it is lossy (the screen can't show it as it is) or
 * of a kind no screen edits.
 */
export function readClause(payload: unknown, prior: ReadPrior = {}): ReadRule | null {
  if (!isObj(payload) || !isObj(payload.body) || typeof payload.kind !== "string") return null;
  const issuedAt = payload.issuedAt;
  if (typeof issuedAt !== "number") return null;
  try {
    const read = back(payload.kind, payload.body, prior);
    if (!read) return null;
    return canonicalJson(forward(read, issuedAt)) === canonicalJson(payload.body) ? read : null;
  } catch {
    return null;
  }
}
