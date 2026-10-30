// The `times` clause — validation and parsing (contract §Times).
//
// The device treats an unparseable or invalid `times` body as ABSENT, and an
// absent clause locks every tagged schedule window ("a named window is a
// restriction relative to Free; opening it as Free would grant exactly what the
// guardian meant to withhold"). So a body this app sends that the device would
// refuse does not fail loudly there — it quietly locks a child out of the very
// time the guardian just set up. Everything on the SEND side therefore throws
// here, at the guardian's own save, rather than shipping a body the device will
// drop. The READ side (`parseGrantTimes`) is the mirror: any fault ⇒ `null`.

import { MAX_NAMED_TIMES, MAX_TIME_LABEL_LEN } from "./grant";
import type { GrantTimes, LearningApp, NamedTime, TimePosture } from "./types";

/** Reserved: Free time has no entry in the clause, so no named time may take
 *  its name (and a gift/ask/window that says "free" means "untyped"). */
export const FREE_TIME_ID = "free";

/** `[a-z0-9-]{1,32}` — mirrors the Rust `valid_time_id`. */
export const TIME_ID_RE = /^[a-z0-9-]{1,32}$/;

/** Is this a usable NAMED-time id? (`"free"` is well-formed but reserved.) */
export function isValidTimeId(s: unknown): s is string {
  return typeof s === "string" && TIME_ID_RE.test(s) && s !== FREE_TIME_ID;
}

/** Label length the way the Rust side counts it (chars, not UTF-16 units). */
function labelLength(s: string): number {
  return [...s].length;
}

function isStringList(v: unknown): v is string[] {
  return Array.isArray(v) && v.every((x) => typeof x === "string");
}

/**
 * Why a site entry would be refused, or `null` if it is fine. The SAME rules
 * as a `learning` site app (contract §Learning: "site apps require `url` +
 * non-empty `domains`; ids must be [a-z0-9-] slugs"), plus `kind: "site"` —
 * a native entry has no resolver pin, so a time that listed one could not
 * constrain the web at all.
 */
export function siteFault(a: unknown): string | null {
  if (typeof a !== "object" || a === null) return "a site entry must be an object";
  const s = a as Record<string, unknown>;
  if (s.kind !== "site") return "only site apps can be listed in a time";
  if (typeof s.id !== "string" || !/^[a-z0-9-]+$/.test(s.id)) {
    return "a site id must be a [a-z0-9-] slug";
  }
  if (typeof s.label !== "string" || s.label.trim().length === 0) return "a site needs a name";
  if (typeof s.url !== "string" || s.url.trim().length === 0) return "a site needs a launch address";
  // The device's `valid_learning_url`: the raw string reaches the resolver sink,
  // so https only, and no whitespace or comma to smuggle a second rule in.
  if (!s.url.startsWith("https://") || /[\s,]/u.test(s.url)) {
    return "a site's launch address must start with https:// and contain no spaces or commas";
  }
  if (!isStringList(s.domains) || s.domains.length === 0 || s.domains.some((d) => d.trim() === "")) {
    return "a site needs at least one verified domain";
  }
  // Hostname characters only (the device's injection guard): `a.org/, EXCLUDE *`
  // would otherwise pass a check that cuts at the first `/`.
  if (s.domains.some((d) => !/^[A-Za-z0-9.-]+$/.test(d))) {
    return "a site's domains must be plain host names";
  }
  return null;
}

/** Why one named time would be refused by the device, or `null` if it is fine. */
export function namedTimeFault(t: unknown): string | null {
  if (typeof t !== "object" || t === null) return "a time must be an object";
  const o = t as Record<string, unknown>;
  if (!isValidTimeId(o.id)) {
    return o.id === FREE_TIME_ID
      ? `"${FREE_TIME_ID}" is reserved for Free time`
      : "a time id must be 1-32 characters of a-z, 0-9 and -";
  }
  if (typeof o.label !== "string" || labelLength(o.label) < 1 || labelLength(o.label) > MAX_TIME_LABEL_LEN) {
    return `a time name must be 1-${MAX_TIME_LABEL_LEN} characters`;
  }
  if (o.posture !== "everythingExcept" && o.posture !== "onlyThese") {
    return "a time needs a posture";
  }
  for (const key of ["blockApps", "unblockApps", "denySites", "undenySites", "apps"] as const) {
    if (o[key] !== undefined && !isStringList(o[key])) return `${key} must be a list of text`;
  }
  if (o.sites !== undefined) {
    if (!Array.isArray(o.sites)) return "sites must be a list";
    // Only the posture in force is validated: the device ignores the other
    // posture's fields, so a stale `sites` on an everythingExcept time is inert.
    if (o.posture === "onlyThese") {
      for (const site of o.sites) {
        const f = siteFault(site);
        if (f) return f;
      }
    }
  }
  if (o.countsAgainstBudget !== undefined && typeof o.countsAgainstBudget !== "boolean") {
    return "countsAgainstBudget must be true or false";
  }
  return null;
}

/** Why a whole `times` set would be refused, or `null` if it is fine. */
export function timesFault(times: unknown): string | null {
  if (!Array.isArray(times)) return "times must be a list";
  if (times.length > MAX_NAMED_TIMES) return `a child can have at most ${MAX_NAMED_TIMES} times`;
  const seen = new Set<string>();
  for (const t of times) {
    const f = namedTimeFault(t);
    if (f) return f;
    const id = (t as NamedTime).id;
    if (seen.has(id)) return `two times share the id "${id}"`;
    seen.add(id);
  }
  return null;
}

function cleanList(list: string[] | undefined): string[] | undefined {
  if (!list) return undefined;
  const out = [...new Set(list.map((s) => s.trim()).filter(Boolean))];
  return out.length > 0 ? out : undefined;
}

function cleanSite(a: LearningApp): LearningApp {
  // Only what a pinned window needs. `free`/`trusted`/`exec` mean something to
  // the `learning` clause and nothing inside a time, so they are not carried.
  return {
    id: a.id,
    label: a.label.trim(),
    kind: "site",
    domains: [...new Set((a.domains ?? []).map((d) => d.trim()).filter(Boolean))],
    url: (a.url ?? "").trim(),
  };
}

/**
 * The canonical wire form of one (already valid) named time: labels and lists
 * trimmed and de-duplicated, empty lists dropped, and ONLY the fields that
 * belong to its posture kept — the device ignores the other posture's fields,
 * so carrying them would be dead weight that could drift the bytes.
 * `countsAgainstBudget` is emitted only when `true` (absent = false).
 */
export function normaliseNamedTime(t: NamedTime): NamedTime {
  const out: NamedTime = { id: t.id, label: t.label.trim(), posture: t.posture as TimePosture };
  if (t.posture === "everythingExcept") {
    const blockApps = cleanList(t.blockApps);
    const unblockApps = cleanList(t.unblockApps);
    const denySites = cleanList(t.denySites);
    const undenySites = cleanList(t.undenySites);
    if (blockApps) out.blockApps = blockApps;
    if (unblockApps) out.unblockApps = unblockApps;
    if (denySites) out.denySites = denySites;
    if (undenySites) out.undenySites = undenySites;
  } else {
    const apps = cleanList(t.apps);
    if (apps) out.apps = apps;
    if (t.sites && t.sites.length > 0) out.sites = t.sites.map(cleanSite);
  }
  if (t.countsAgainstBudget === true) out.countsAgainstBudget = true;
  return out;
}

/**
 * Read a `times` clause body back. Fail-closed exactly as the device is: any
 * fault anywhere, or a `v` other than 1, gives `null` ("treated as absent"),
 * never a best-effort partial set — a partial set would show the guardian
 * times the ward is not actually running.
 */
export function parseGrantTimes(body: unknown): GrantTimes | null {
  if (typeof body !== "object" || body === null) return null;
  const o = body as Record<string, unknown>;
  if (o.v !== 1) return null;
  if (typeof o.issuedAt !== "number" || !Number.isInteger(o.issuedAt) || o.issuedAt < 0) return null;
  if (timesFault(o.times) !== null) return null;
  return {
    v: 1,
    times: (o.times as NamedTime[]).map(normaliseNamedTime),
    issuedAt: o.issuedAt,
  };
}
