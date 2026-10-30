// Generate schedule parity vectors from the SDK's canonical evaluateSchedule.
// node CANNOT import the .ts source, so we import from dist/ — run `npm run
// build` first.
//
// Usage (from repo root):
//   npm run build && node scripts/gen-schedule-vectors.mjs
//
// Output: core/crates/charter-testkit/vectors/schedule/schedule_vectors.json
// Each vector pins the live TS result so the Rust port must match exactly.

import { evaluateSchedule } from "../dist/schedule/index.js";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = join(__dirname, "..", "core", "crates", "charter-testkit", "vectors", "schedule");
mkdirSync(OUT_DIR, { recursive: true });

const clause = (windows, opts = {}) => ({
  schemaVersion: 1,
  kind: "schedule",
  mechanism: "static-data",
  windows,
  timezone: opts.timezone ?? "Europe/London",
  endDate: opts.endDate ?? null,
  revoked: opts.revoked ?? false,
});

// 0=Sun..6=Sat; minutes are minute-of-day in the clause tz.
const WEEKDAYS = [1, 2, 3, 4, 5];
const afterSchool = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }]); // 16:00-20:00
const fridayNight = clause([{ daysOfWeek: [5], startMinute: 1320, endMinute: 360 }]); // Fri 22:00 -> Sat 06:00 (crosses midnight)
const twoWindows = clause([
  { daysOfWeek: [6, 0], startMinute: 540, endMinute: 720 }, // weekend 09:00-12:00
  { daysOfWeek: [6, 0], startMinute: 840, endMinute: 1080 }, // weekend 14:00-18:00
]);
const empty = clause([]); // always allow
const revoked = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }], { revoked: true });
const expired = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }], { endDate: 1000 });
const badTz = clause([{ daysOfWeek: WEEKDAYS, startMinute: 960, endMinute: 1200 }], { timezone: "Not/AZone" });

const cases = [
  // After-school weekday window, BST (June) and GMT (January).
  ["weekday_in_window_bst", afterSchool, "2026-06-29T17:00:00Z"], // Mon 18:00 BST -> allow
  ["weekday_before_window_bst", afterSchool, "2026-06-29T14:00:00Z"], // Mon 15:00 BST -> lock
  ["weekday_in_window_gmt", afterSchool, "2026-01-26T17:00:00Z"], // Mon 17:00 GMT -> allow
  ["weekday_after_window_gmt", afterSchool, "2026-01-26T20:30:00Z"], // Mon 20:30 GMT -> lock
  ["weekend_not_in_weekday_window", afterSchool, "2026-06-28T17:00:00Z"], // Sun -> lock
  // Midnight-crossing Friday-night window.
  ["friday_2300_in_window", fridayNight, "2026-06-26T22:00:00Z"], // Fri 23:00 BST -> allow
  ["saturday_0500_in_window", fridayNight, "2026-06-27T04:00:00Z"], // Sat 05:00 BST -> allow
  ["saturday_0700_out_of_window", fridayNight, "2026-06-27T06:30:00Z"], // Sat 07:30 BST -> lock
  // Two windows same day.
  ["weekend_first_window", twoWindows, "2026-06-27T09:00:00Z"], // Sat 10:00 BST -> allow
  ["weekend_gap_locked", twoWindows, "2026-06-27T12:00:00Z"], // Sat 13:00 BST -> lock
  ["weekend_second_window", twoWindows, "2026-06-27T15:00:00Z"], // Sat 16:00 BST -> allow
  // Special clauses.
  ["empty_always_allow", empty, "2026-06-29T03:00:00Z"],
  ["revoked_blocks", revoked, "2026-06-29T17:00:00Z"],
  ["expired_blocks", expired, "2026-06-29T17:00:00Z"],
  ["bad_tz_fails_open", badTz, "2026-06-29T03:00:00Z"], // malformed tz -> allow no_clause
];

const vectors = cases.map(([name, c, iso]) => {
  const now = new Date(iso);
  return {
    name,
    clause: c,
    now_unix: Math.floor(now.getTime() / 1000),
    expect: evaluateSchedule(c, now),
  };
});

const path = join(OUT_DIR, "schedule_vectors.json");
writeFileSync(path, JSON.stringify({ vectors }, null, 2) + "\n");
console.log("wrote", path, `(${vectors.length} vectors)`);

// ---------------------------------------------------------------------------
// Times (spec/contract.md §Times). Two files, both pinned HERE: the TS SDK has
// no tagged-window evaluator (the `src/` schedule shape is the older
// ChartedClause), so this script carries its own small reference implementation
// and the Rust port (charter-schedule::times, charter-proto::times) must agree
// with it. The reference deliberately uses a DIFFERENT algorithm from the Rust
// one (a per-minute sweep rather than boundary search), so an off-by-one in
// either shows up as a disagreement.
//
//   schedule/times_schedule_vectors.json  tagged-window evaluation (UTC only,
//                                         so every instant is hand-checkable)
//   times/times_clause_vectors.json       parse/validate verdicts, composition,
//                                         and the additive `time` fields
// ---------------------------------------------------------------------------

const TIMES_DIR = join(__dirname, "..", "core", "crates", "charter-testkit", "vectors", "times");
mkdirSync(TIMES_DIR, { recursive: true });

// Mon 2026-06-29 00:00:00 UTC.
const MON = 1782691200;
const at = (h, m = 0, dayOffset = 0) => MON + dayOffset * 86400 + h * 3600 + m * 60;
const DAYS = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const hhmm = (min) => `${String(Math.floor(min / 60)).padStart(2, "0")}:${String(min % 60).padStart(2, "0")}`;
const toMin = (s) => Number(s.slice(0, 2)) * 60 + Number(s.slice(3, 5));

const win = (start, end, time) => (time === undefined ? { start, end } : { start, end, time });
const sched = (mon, extra = {}) => ({ v: 1, tz: "Etc/UTC", weekly: { mon, ...(extra.weekly ?? {}) }, ...(extra.overrides ? { overrides: extra.overrides } : {}), ...(extra.paused ? { paused: true } : {}), issuedAt: 1782734400 });
const namedTime = (id, label, posture, rest = {}) => ({ id, label, posture, ...rest });
const timesBody = (times) => ({ v: 1, issuedAt: 1782734400, times });

// Reference evaluation of one local UTC day, minute by minute.
function dayWindows(schedule, dayStart) {
  const d = new Date(dayStart * 1000);
  const key = d.toISOString().slice(0, 10);
  const raw = schedule.overrides?.[key] ?? schedule.weekly?.[DAYS[d.getUTCDay()]] ?? [];
  return raw
    .map((w) => ({ s: toMin(w.start), e: toMin(w.end), tag: w.time ?? null }))
    .filter((w) => w.s < w.e);
}
// label: "closed" | "free" | "t:<id>" | "u:<id>" (tagged, unknown)
function labelAt(wins, known, minute) {
  let best = null;
  for (const w of wins) {
    if (w.tag !== null && minute >= w.s && minute < w.e && (best === null || w.s < best.s)) best = w;
  }
  if (best) return known.has(best.tag) ? `t:${best.tag}` : `u:${best.tag}`;
  return wins.some((w) => w.tag === null && minute >= w.s && minute < w.e) ? "free" : "closed";
}
const runnable = (l) => l === "free" || l.startsWith("t:");
const labelTime = (l) => (l.startsWith("t:") ? l.slice(2) : null);

function refTimes(schedule, timesClause, nowUnix) {
  const known = new Set((timesClause?.times ?? []).map((t) => t.id));
  const dayStart = nowUnix - (nowUnix % 86400);
  const minute = Math.floor((nowUnix - dayStart) / 60);
  const hasAny =
    Object.values(schedule.weekly ?? {}).some((w) => w.length > 0) || Object.keys(schedule.overrides ?? {}).length > 0;
  const out = { expect: null, nextWindow: null, defaultAsk: null };
  if (schedule.paused) {
    out.expect = { state: "locked", cause: "paused", secondsToOpen: null };
    return out;
  }
  if (!hasAny) {
    out.expect = { state: "unbounded" };
    return out;
  }
  const wins = dayWindows(schedule, dayStart);
  const cur = labelAt(wins, known, minute);
  if (runnable(cur)) {
    let m = minute + 1;
    while (m < 1440 && labelAt(wins, known, m) === cur) m++;
    out.expect = { state: "open", time: labelTime(cur), spanEnd: dayStart + m * 60 };
  } else {
    let secondsToOpen = null;
    scan: for (let day = 0; day < 8; day++) {
      const ds = dayStart + day * 86400;
      const dw = dayWindows(schedule, ds);
      for (let m = day === 0 ? minute + 1 : 0; m < 1440; m++) {
        if (runnable(labelAt(dw, known, m))) {
          secondsToOpen = ds + m * 60 - nowUnix;
          break scan;
        }
      }
    }
    out.expect = {
      state: "locked",
      cause: cur.startsWith("u:") ? "unknownTime" : "outsideWindows",
      ...(cur.startsWith("u:") ? { unknownTime: cur.slice(2) } : {}),
      secondsToOpen,
    };
  }
  // Next runnable span to START today or tomorrow, strictly after now.
  next: for (let day = 0; day <= 1; day++) {
    const ds = dayStart + day * 86400;
    const dw = dayWindows(schedule, ds);
    for (let m = day === 0 ? minute + 1 : 0; m < 1440; m++) {
      const l = labelAt(dw, known, m);
      if (runnable(l) && (m === 0 || labelAt(dw, known, m - 1) !== l)) {
        out.nextWindow = { time: labelTime(l), start: ds + m * 60, startHhmm: hhmm(m), dayOffset: day };
        break next;
      }
    }
  }
  // Default ask type: most recently ended runnable span today, else next up, else Free (null).
  const spans = [];
  for (let m = 0; m < 1440; ) {
    const l = labelAt(wins, known, m);
    let e = m + 1;
    while (e < 1440 && labelAt(wins, known, e) === l) e++;
    if (runnable(l)) spans.push({ s: m, e, time: labelTime(l) });
    m = e;
  }
  const ended = spans.filter((sp) => sp.e <= minute).sort((a, b) => b.e - a.e)[0];
  const upcoming = spans.filter((sp) => sp.s > minute).sort((a, b) => a.s - b.s)[0];
  out.defaultAsk = (ended ?? upcoming)?.time ?? null;
  return out;
}

const LEARNING = namedTime("learning", "Learning time", "onlyThese", { apps: ["org.khanacademy"] });
const CHORES = namedTime("chores", "Chores", "everythingExcept");
const clauseLearning = timesBody([LEARNING]);
const clauseBoth = timesBody([LEARNING, CHORES]);
const freeCarved = sched([win("16:00", "19:00"), win("17:00", "18:00", "learning")]);

// [name, schedule, times clause body | null (none) | {invalid body}, now_unix]
const timesCases = [
  ["untagged_in_window", sched([win("16:00", "19:00")]), null, at(16, 30)],
  ["untagged_before_window_locked", sched([win("16:00", "19:00")]), clauseLearning, at(15, 0)],
  ["carve_before_edge_is_free_until_the_carve", freeCarved, clauseLearning, at(16, 30)],
  ["carve_start_instant_is_learning", freeCarved, clauseLearning, at(17, 0)],
  ["carve_inside_is_learning", freeCarved, clauseLearning, at(17, 59)],
  ["carve_end_instant_is_free_again", freeCarved, clauseLearning, at(18, 0)],
  ["carve_with_no_times_clause_locks_the_span", freeCarved, null, at(17, 30)],
  ["carve_with_no_times_clause_leaves_free_alone", freeCarved, null, at(16, 30)],
  ["carve_with_other_ids_only_locks_the_span", freeCarved, timesBody([CHORES]), at(17, 30)],
  ["carve_with_unreadable_times_clause_locks_the_span", freeCarved, { v: 2, issuedAt: 1, times: [LEARNING] }, at(17, 30)],
  ["named_window_alone_opens_that_time", sched([win("09:00", "12:00", "learning")]), clauseLearning, at(10, 15)],
  ["named_window_alone_unknown_is_locked", sched([win("09:00", "12:00", "learning")]), null, at(10, 15)],
  ["named_window_alone_then_locked_tomorrow_next", sched([win("09:00", "12:00", "learning")], { weekly: { tue: [win("09:00", "12:00", "learning")] } }), clauseLearning, at(13, 0)],
  ["tagged_overlap_earlier_start_wins", sched([win("10:00", "12:00", "chores"), win("09:00", "11:00", "learning")]), clauseBoth, at(10, 30)],
  ["tagged_overlap_later_one_resumes_after", sched([win("10:00", "12:00", "chores"), win("09:00", "11:00", "learning")]), clauseBoth, at(11, 30)],
  ["tagged_overlap_same_start_array_order", sched([win("09:00", "10:00", "chores"), win("09:00", "11:00", "learning")]), clauseBoth, at(9, 30)],
  ["tagged_overlap_same_start_longer_resumes", sched([win("09:00", "10:00", "chores"), win("09:00", "11:00", "learning")]), clauseBoth, at(10, 30)],
  ["adjacent_same_time_windows_are_one_span", sched([win("09:00", "10:00"), win("10:00", "11:00")]), null, at(9, 30)],
  ["adjacent_same_named_time_windows_are_one_span", sched([win("09:00", "10:00", "learning"), win("10:00", "11:00", "learning")]), clauseLearning, at(9, 30)],
  ["adjacent_different_times_split_the_span", sched([win("09:00", "10:00", "learning"), win("10:00", "11:00", "chores")]), clauseBoth, at(9, 30)],
  ["gap_between_same_time_windows_ends_the_span", sched([win("09:00", "10:00"), win("11:00", "12:00")]), null, at(9, 30)],
  ["override_replaces_weekly_and_keeps_its_tag", sched([win("09:00", "12:00")], { overrides: { "2026-06-29": [win("13:00", "14:00", "learning")] } }), clauseLearning, at(13, 30)],
  ["override_replaces_weekly_so_weekly_hours_are_locked", sched([win("09:00", "12:00")], { overrides: { "2026-06-29": [win("13:00", "14:00", "learning")] } }), clauseLearning, at(10, 0)],
  ["override_empty_blocks_the_day", sched([win("09:00", "12:00")], { overrides: { "2026-06-29": [] } }), clauseLearning, at(10, 0)],
  ["day_with_no_entry_is_locked", sched([win("09:00", "12:00")]), clauseLearning, at(10, 0, 1)],
  ["last_minute_of_the_day_span_ends_at_its_window", sched([win("22:00", "23:59")]), null, at(23, 0)],
  ["paused_schedule_is_locked", sched([win("09:00", "12:00", "learning")], { paused: true }), clauseLearning, at(10, 0)],
  ["empty_schedule_is_unbounded", { v: 1, tz: "Etc/UTC", weekly: {}, issuedAt: 1782734400 }, clauseLearning, at(3, 0)],
  ["default_ask_before_anything_is_the_next_up", sched([win("09:00", "12:00", "learning"), win("16:00", "19:00")]), clauseLearning, at(7, 0)],
  ["default_ask_after_learning_ended_is_learning", sched([win("09:00", "12:00", "learning"), win("16:00", "19:00")]), clauseLearning, at(13, 0)],
  ["default_ask_after_free_ended_is_free", sched([win("09:00", "12:00", "learning"), win("16:00", "19:00")]), clauseLearning, at(20, 0)],
  ["default_ask_ignores_a_span_that_cannot_run", sched([win("09:00", "12:00", "learning"), win("16:00", "19:00")]), null, at(13, 0)],
  ["next_window_skips_an_unknown_tag", sched([win("16:00", "19:00"), win("20:00", "21:00", "mystery")]), clauseLearning, at(19, 30)],
];

const timesScheduleVectors = timesCases.map(([name, schedule, times, now]) => {
  const r = refTimes(schedule, times && times.v === 1 ? times : null, now);
  return { name, schedule, times, now_unix: now, expect: r.expect, nextWindow: r.nextWindow, defaultAsk: r.defaultAsk };
});
const timesSchedPath = join(OUT_DIR, "times_schedule_vectors.json");
writeFileSync(
  timesSchedPath,
  JSON.stringify(
    {
      note: "Tagged-window schedule evaluation (spec/contract.md §Times). `schedule` is a runtime GrantSchedule body, `times` the device's times clause body (null = none; a body with v != 1 is UNREADABLE and so absent). All schedules are Etc/UTC so spanEnd and secondsToOpen are plain arithmetic. A tagged window beats an untagged one; two tagged windows resolve to the earlier start, then array order; a tag the clause does not define LOCKS its span. nextWindow is the next runnable span to start today or tomorrow; defaultAsk is the ward's default ask type (null = Free time).",
      vectors: timesScheduleVectors,
    },
    null,
    2,
  ) + "\n",
);
console.log("wrote", timesSchedPath, `(${timesScheduleVectors.length} vectors)`);

// ----- times clause: parse/validate verdicts ---------------------------------

const site = (id, domains, extra = {}) => ({ id, label: id, kind: "site", url: `https://${domains[0]}/`, domains, ...extra });
const KHAN = site("khan-academy", ["khanacademy.org", "kastatic.org", "kasandbox.org"]);
const YT = site("youtube", ["youtube.com", "googlevideo.com", "ytimg.com"]);
const bt = (id, extra = {}) => ({ id, label: `Time ${id}`, posture: "onlyThese", ...extra });

const validClauses = [
  ["empty_clause", timesBody([]), {}],
  ["learning_template_runs_nothing", timesBody([namedTime("learning", "Learning time", "onlyThese", { countsAgainstBudget: false })]), { ids: ["learning"], runsNothing: ["learning"], countsAgainstBudget: [] }],
  ["only_these_apps_and_sites", timesBody([namedTime("learning", "Learning time", "onlyThese", { apps: ["org.khanacademy"], sites: [KHAN, YT] })]), { ids: ["learning"], runsNothing: [], countsAgainstBudget: [] }],
  ["everything_except_with_departures", timesBody([namedTime("chores", "Chores", "everythingExcept", { blockApps: ["roblox"], unblockApps: ["youtube"], denySites: ["roblox.com"], undenySites: ["youtube.com"], countsAgainstBudget: true })]), { ids: ["chores"], runsNothing: [], countsAgainstBudget: ["chores"] }],
  ["fields_of_the_inactive_posture_are_ignored", timesBody([namedTime("chores", "Chores", "everythingExcept", { sites: [{ id: "BAD ID", label: "x", kind: "native" }], apps: ["x"] })]), { ids: ["chores"], runsNothing: [], countsAgainstBudget: [] }],
  ["max_times", timesBody(Array.from({ length: 16 }, (_, i) => bt(`t${i}`))), { ids: Array.from({ length: 16 }, (_, i) => `t${i}`), runsNothing: Array.from({ length: 16 }, (_, i) => `t${i}`), countsAgainstBudget: [] }],
  ["id_of_32_chars", timesBody([bt("a".repeat(32))]), { ids: ["a".repeat(32)], runsNothing: ["a".repeat(32)], countsAgainstBudget: [] }],
  ["label_of_60_characters", timesBody([bt("a", { label: "é".repeat(60) })]), { ids: ["a"], runsNothing: ["a"], countsAgainstBudget: [] }],
  ["id_with_digits_and_hyphens", timesBody([bt("homework-2")]), { ids: ["homework-2"], runsNothing: ["homework-2"], countsAgainstBudget: [] }],
];
const invalidClauses = [
  ["v_too_high", { v: 2, issuedAt: 1, times: [] }],
  ["v_zero", { v: 0, issuedAt: 1, times: [] }],
  ["times_not_an_array", { v: 1, issuedAt: 1, times: {} }],
  ["missing_issuedAt", { v: 1, times: [] }],
  ["seventeen_times", timesBody(Array.from({ length: 17 }, (_, i) => bt(`t${i}`)))],
  ["free_is_reserved", timesBody([bt("free")])],
  ["empty_id", timesBody([bt("")])],
  ["upper_case_id", timesBody([bt("Learning")])],
  ["id_with_space", timesBody([bt("has space")])],
  ["id_with_underscore", timesBody([bt("under_score")])],
  ["id_of_33_chars", timesBody([bt("a".repeat(33))])],
  ["duplicate_ids", timesBody([bt("a"), bt("a", { posture: "everythingExcept" })])],
  ["empty_label", timesBody([bt("a", { label: "" })])],
  ["label_of_61_characters", timesBody([bt("a", { label: "é".repeat(61) })])],
  ["unknown_posture", timesBody([bt("a", { posture: "mostly" })])],
  ["missing_posture", timesBody([{ id: "a", label: "A" }])],
  ["site_without_url", timesBody([bt("a", { sites: [{ id: "x", label: "x", kind: "site", domains: ["a.org"] }] })])],
  ["site_without_domains", timesBody([bt("a", { sites: [site("x", ["a.org"], { domains: [] })] })])],
  ["site_url_not_https", timesBody([bt("a", { sites: [site("x", ["a.org"], { url: "http://a.org/" })] })])],
  ["site_url_with_comma", timesBody([bt("a", { sites: [site("x", ["a.org"], { url: "https://a.org/,x" })] })])],
  ["site_domain_injection", timesBody([bt("a", { sites: [site("x", ["a.org/, EXCLUDE *"])] })])],
  ["site_id_not_a_slug", timesBody([bt("a", { sites: [site("X y", ["a.org"])] })])],
  ["site_of_kind_native", timesBody([bt("a", { sites: [{ id: "gcompris", label: "GCompris", kind: "native", exec: "/usr/bin/gcompris-qt" }] })])],
];

// ----- composition -----------------------------------------------------------

const normDomain = (raw) => {
  let s = raw.trim().toLowerCase();
  for (const scheme of ["https://", "http://"]) if (s.startsWith(scheme)) s = s.slice(scheme.length);
  const cut = s.search(/[/?:]/);
  s = (cut === -1 ? s : s.slice(0, cut)).replace(/\.+$/, "");
  return s.startsWith("www.") ? s.slice(4) : s;
};
const uniq = (xs, key = (x) => x) => {
  const seen = new Set();
  return xs.filter((x) => (seen.has(key(x)) ? false : (seen.add(key(x)), true)));
};
const freeApps = (o = {}) => ({ v: 1, posture: "blocklist", blocked: ["minecraft", "youtube"], allowed: [], askFirst: [], hidden: [], issuedAt: 1782734400, ...o });
const freeContent = (o = {}) => ({ v: 1, posture: "blocklist", ageTier: "older", curators: [], blockCategories: [], parentAllow: [], parentDeny: [], issuedAt: 1782734400, ...o });

function refComposeApps(free, t) {
  const out = JSON.parse(JSON.stringify(free));
  const has = (xs, x) => xs.includes(x);
  if (t.posture === "everythingExcept") {
    const block = t.blockApps ?? [];
    const unblock = t.unblockApps ?? [];
    out.blocked = uniq([...free.blocked, ...block].filter((p) => !has(unblock, p)));
    if (free.posture === "allowlist") out.allowed = uniq([...free.allowed.filter((p) => !has(block, p)), ...unblock]);
    out.askFirst = (free.askFirst ?? []).filter((p) => has(out.blocked, p));
  } else {
    out.posture = "allowlist";
    out.allowed = uniq(t.apps ?? []);
    out.blocked = [];
    out.askFirst = [];
    delete out.paused;
    out.holds = [];
  }
  return out;
}
function refComposeContent(free, t) {
  const synth = (posture) => ({ v: 1, posture, ageTier: "young", curators: [], blockCategories: [], parentAllow: [], parentDeny: [], issuedAt: 0 });
  if (t.posture === "everythingExcept") {
    const deny = t.denySites ?? [];
    const undeny = t.undenySites ?? [];
    if (deny.length === 0 && undeny.length === 0) return free;
    let out;
    if (free) out = JSON.parse(JSON.stringify(free));
    else if (deny.length > 0) out = synth("blocklist");
    else return null;
    const undenyKeys = undeny.map(normDomain);
    out.parentDeny = uniq([...out.parentDeny, ...deny].filter((d) => !undenyKeys.includes(normDomain(d))), normDomain);
    out.parentAllow = uniq([...out.parentAllow, ...undeny], normDomain);
    return out;
  }
  const sites = t.sites ?? [];
  if (sites.length === 0) return free;
  let out;
  if (free) out = JSON.parse(JSON.stringify(free));
  else {
    out = synth("allowlist");
    out.safeSearch = true;
  }
  const listed = uniq(sites.flatMap((s) => s.domains), normDomain);
  const keys = listed.map(normDomain);
  out.posture = "allowlist";
  out.curators = [];
  delete out.revoked;
  out.parentDeny = out.parentDeny.filter((d) => !keys.includes(normDomain(d)));
  out.parentAllow = listed;
  return out;
}

const compCases = [
  ["every_except_ban_and_unban_apps", freeApps({ askFirst: ["youtube"], hidden: ["bloat"] }), null, namedTime("chores", "Chores", "everythingExcept", { blockApps: ["roblox"], unblockApps: ["youtube"] })],
  ["every_except_unban_wins_over_same_time_ban", freeApps(), null, namedTime("chores", "Chores", "everythingExcept", { blockApps: ["youtube"], unblockApps: ["youtube"] })],
  ["every_except_inherits_free_bans_live", freeApps({ blocked: ["minecraft", "youtube", "tiktok"] }), null, namedTime("chores", "Chores", "everythingExcept", {})],
  ["every_except_over_allowlist_free_edits_allowed", freeApps({ posture: "allowlist", blocked: [], allowed: ["khan", "calc"] }), null, namedTime("chores", "Chores", "everythingExcept", { blockApps: ["calc"], unblockApps: ["paint"] })],
  ["every_except_sites_normalise_and_unban_into_parentAllow", freeApps(), freeContent({ parentDeny: ["https://www.YouTube.com/", "tiktok.com"], safeSearch: false, youtubeRestrict: "strict" }), namedTime("chores", "Chores", "everythingExcept", { denySites: ["roblox.com"], undenySites: ["youtube.com"] })],
  ["every_except_without_site_departures_returns_free_content_as_is", freeApps(), freeContent({ parentDeny: ["tiktok.com"] }), namedTime("chores", "Chores", "everythingExcept", { blockApps: ["roblox"] })],
  ["every_except_deny_with_no_free_content_synthesises_a_blocklist", freeApps(), null, namedTime("chores", "Chores", "everythingExcept", { denySites: ["roblox.com"] })],
  ["every_except_undeny_with_no_free_content_stays_none", freeApps(), null, namedTime("chores", "Chores", "everythingExcept", { undenySites: ["youtube.com"] })],
  ["only_these_apps_make_an_allowlist", freeApps({ hidden: ["bloat"] }), freeContent({ parentDeny: ["tiktok.com"] }), namedTime("learning", "Learning", "onlyThese", { apps: ["khan", "khan", "calc"] })],
  ["only_these_clears_a_paused_apps_policy", freeApps({ paused: true }), null, namedTime("learning", "Learning", "onlyThese", { apps: ["khan"] })],
  ["only_these_with_nothing_runs_nothing", freeApps(), freeContent({ parentDeny: ["tiktok.com"] }), namedTime("learning", "Learning", "onlyThese", {})],
  ["only_these_sites_make_a_web_allowlist", freeApps(), freeContent({ curators: ["a".repeat(64)], parentAllow: ["old.org"], parentDeny: ["tiktok.com", "kastatic.org"], safeSearch: false, youtubeRestrict: "moderate", revoked: true }), namedTime("learning", "Learning", "onlyThese", { apps: ["khan"], sites: [KHAN, YT] })],
  ["only_these_sites_with_no_free_content_synthesise_an_allowlist", freeApps(), null, namedTime("learning", "Learning", "onlyThese", { sites: [KHAN] })],
  ["only_these_sites_dedupe_shared_domains", freeApps(), freeContent(), namedTime("learning", "Learning", "onlyThese", { sites: [KHAN, site("khan-kids", ["khanacademy.org", "khankids.org"])] })],
  ["fields_of_the_inactive_posture_are_not_composed", freeApps(), freeContent(), namedTime("chores", "Chores", "everythingExcept", { apps: ["khan"], sites: [KHAN] })],
];
const compositionVectors = compCases.map(([name, fa, fc, t]) => ({
  name,
  freeApps: fa,
  freeContent: fc,
  time: t,
  expectApps: refComposeApps(fa, t),
  expectContent: refComposeContent(fc, t),
}));

// ----- the additive `time` fields on gift and time.extend --------------------

const giftBody = (extra = {}) => ({ v: 1, issuedAt: 1782734400, id: "gift-1", minutes: 30, expiresAt: 1782777600, ...extra });
const paramVectors = [
  { name: "gift_without_time_is_byte_identical", kind: "gift", payload: giftBody(), expectTime: null },
  { name: "gift_with_time", kind: "gift", payload: giftBody({ time: "learning" }), expectTime: "learning" },
  { name: "gift_with_unknown_looking_time_still_parses", kind: "gift", payload: giftBody({ time: "no-such-time" }), expectTime: "no-such-time" },
  { name: "gift_with_group_and_time", kind: "gift", payload: giftBody({ groupId: "play", time: "learning" }), expectTime: "learning" },
  { name: "request_without_time_is_byte_identical", kind: "time.extend.request", payload: { minutesRequested: 30, limitHit: "budget" }, expectTime: null },
  { name: "request_with_time", kind: "time.extend.request", payload: { minutesRequested: 30, limitHit: "schedule", time: "learning" }, expectTime: "learning" },
  { name: "grant_without_time_is_byte_identical", kind: "time.extend.grant", payload: { minutesGranted: 30, limitHit: "budget" }, expectTime: null },
  { name: "grant_echoes_time_verbatim", kind: "time.extend.grant", payload: { minutesGranted: 30, limitHit: "schedule", time: "learning" }, expectTime: "learning" },
  { name: "zero_grant_closing_an_ask_can_carry_time", kind: "time.extend.grant", payload: { minutesGranted: 0, limitHit: "schedule", time: "learning" }, expectTime: "learning" },
];

const timesClausePath = join(TIMES_DIR, "times_clause_vectors.json");
writeFileSync(
  timesClausePath,
  JSON.stringify(
    {
      note: "Canonical times clause vectors (spec/contract.md §Times) — pins the guardian producer ↔ warden (Rust: GrantTimes parse/validate, compose_apps, compose_content). `vectors` must parse and validate; `invalid` must fail closed (the clause is then ABSENT: every tagged window locks). `composition` pins Free time's resolved standing policy + one named time → the policy in force (expectContent null = no content clause). `params` pins the additive `time` field on gift and time.extend request/grant: absent stays byte-identical, present round-trips verbatim.",
      vectors: validClauses.map(([name, payload, expect]) => ({ name, payload, expect })),
      invalid: invalidClauses.map(([name, payload]) => ({ name, payload })),
      composition: compositionVectors,
      params: paramVectors,
    },
    null,
    2,
  ) + "\n",
);
console.log("wrote", timesClausePath, `(${validClauses.length} valid, ${invalidClauses.length} invalid, ${compositionVectors.length} composition, ${paramVectors.length} params)`);
