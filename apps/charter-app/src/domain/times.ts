// Times — Free time, Learning time and other named times (contract §Times,
// docs/times-design.md). The guardian-side domain layer: what the Time editor,
// the Now card and the More-time picker are built on.
//
// THE MODEL, in the four sentences that matter here. Free time is the
// background and has no entry anywhere — it is the standing clauses, and an
// UNTAGGED `ScheduleWindow` runs it. A named time is DEFINED once per child
// (`Policy.times`, the `times` clause) and RUN by a `ScheduleWindow` tagged
// with its id in each device's own schedule. A device is in exactly one time at
// any instant, or locked. A tag the child's `times` does not define LOCKS its
// span, because a named window is a restriction relative to Free and opening
// it as Free would grant exactly what the guardian meant to withhold.
//
// That last rule is the fail direction for everything below: when this file is
// unsure it answers "locked", never "free".
//
// This module is not the same thing as `./namedTimes.ts`, which is the OLDER
// "named times" concept (groups of apps: time-free / counted / on request) and
// is untouched; a *time* in this file always means a window with a posture.

import { MAX_NAMED_TIMES, MAX_TIME_LABEL_LEN, endOfDayUnix, startOfDayUnix, tzDayParts } from "../wire/grant";
import { FREE_TIME_ID, isValidTimeId, namedTimeFault } from "../wire/times";
import type { DeviceStatus } from "../wire/status";
import type { LearningApp } from "../wire/types";
import { setControlSplit, setDeviceControl } from "./effectivePolicy";
import type { Device, NamedTime, Policy, Schedule, ScheduleWindow, Weekday } from "./types";
import { WEEKDAYS } from "./types";
import { wardenSupport } from "./wardenSupport";

// ---------------------------------------------------------------------------
// Clock helpers (the schedule's own tz — never the guardian phone's)
// ---------------------------------------------------------------------------

const HHMM = /^([01]\d|2[0-3]):([0-5]\d)$/;

function minutesOf(hhmm: string): number {
  return Number(hhmm.slice(0, 2)) * 60 + Number(hhmm.slice(3, 5));
}

/** Calendar day + seconds-into-day in `tz`; an unknown tz falls back to the
 *  runtime's local one, as every other tz reader in this app does. */
function localParts(atSecs: number, tz: string): { dayKey: string; secsIntoDay: number } {
  try {
    return tzDayParts(atSecs, tz);
  } catch {
    return tzDayParts(atSecs);
  }
}

const SUNDAY_FIRST: Weekday[] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

function weekdayOf(dayKey: string): Weekday {
  const [y, m, d] = dayKey.split("-").map(Number);
  return SUNDAY_FIRST[new Date(Date.UTC(y, m - 1, d)).getUTCDay()];
}

/** The windows in force on a calendar day: the date's override if there is one
 *  (an empty override is a closed day, not a missing one), else the weekly. */
function windowsOn(schedule: Schedule, dayKey: string): ScheduleWindow[] {
  const o = schedule.overrides;
  if (o && Object.prototype.hasOwnProperty.call(o, dayKey)) return o[dayKey];
  return schedule.weekly[weekdayOf(dayKey)] ?? [];
}

/** Unix seconds of `secsOfDay` on the tz-local day that starts at `dayStart`.
 *  Re-reads the candidate and nudges it onto the wall-clock time, so a DST day
 *  (23h/25h) lands on the right hour rather than an hour out. */
function unixAtLocal(dayStart: number, secsOfDay: number, tz: string): number {
  const day = localParts(dayStart, tz).dayKey;
  let at = dayStart + secsOfDay;
  for (let i = 0; i < 2; i += 1) {
    const p = localParts(at, tz);
    if (p.dayKey !== day || p.secsIntoDay === secsOfDay) break;
    at += secsOfDay - p.secsIntoDay;
  }
  return at;
}

const inside = (w: ScheduleWindow, secs: number): boolean =>
  minutesOf(w.start) * 60 <= secs && secs < minutesOf(w.end) * 60;

/** A tagged window whose tag the child has not defined (so its span is locked). */
const isKnown = (w: ScheduleWindow, times: readonly NamedTime[]): boolean =>
  w.time === undefined || times.some((t) => t.id === w.time);

// ---------------------------------------------------------------------------
// Which time is it now? (mirrors the device rule — see the contract)
// ---------------------------------------------------------------------------

/**
 * What a device is in at an instant, by its schedule and the child's times.
 *
 *  - `free`: inside an untagged window, or the schedule is absent / lifted.
 *  - `time`: inside a tagged window (or a pool-run, which only the device
 *    knows — STATUS `activeTime` is the truth for the Now card; this is its
 *    schedule-only mirror).
 *  - `locked / outside`: no window covers now.
 *  - `locked / unknown-time`: the winning tagged window names a time the child
 *    has no definition for. Reported distinctly because the ward is told
 *    "Nothing is set up for this time yet", not the ordinary schedule line.
 */
export type ActiveTime =
  | { state: "free" }
  | { state: "time"; time: NamedTime }
  | { state: "locked"; reason: "outside" }
  | { state: "locked"; reason: "unknown-time"; tag: string };

/**
 * The device's own rule, run guardian-side: take today's windows (the date's
 * override, else the weekly); inside a TAGGED window ⇒ that time (two of them:
 * earlier `start` wins, then array order — the editor never emits the overlap
 * but a device can still receive one); else inside an UNTAGGED window ⇒ Free;
 * else locked. A tag with no definition ⇒ locked, never Free.
 *
 * A schedule that is absent, or "lifted" (`paused`: the app's off-switch, wire
 * "no limit"), is Free all day — the same reading `scheduleToGrant` gives it.
 */
export function activeTimeAt(
  schedule: Schedule | undefined,
  times: readonly NamedTime[] | undefined,
  nowSecs: number,
): ActiveTime {
  if (!schedule || schedule.paused) return { state: "free" };
  const { dayKey, secsIntoDay } = localParts(nowSecs, schedule.tz);
  const covering = windowsOn(schedule, dayKey).filter((w) => inside(w, secsIntoDay));
  let tagged: ScheduleWindow | undefined;
  for (const w of covering) {
    if (w.time === undefined) continue;
    // Strictly earlier only, so equal starts keep array order.
    if (!tagged || minutesOf(w.start) < minutesOf(tagged.start)) tagged = w;
  }
  if (tagged) {
    const time = (times ?? []).find((t) => t.id === tagged!.time);
    return time
      ? { state: "time", time }
      : { state: "locked", reason: "unknown-time", tag: tagged.time as string };
  }
  return covering.length > 0 ? { state: "free" } : { state: "locked", reason: "outside" };
}

/**
 * Which time "More time" should offer first, as an id (`undefined` = Free).
 * The ward-side rule from the contract: the time whose window most recently
 * ENDED today, else the next one still to come today, else Free.
 *
 * One addition the contract leaves open: a device that is inside a window right
 * now is already in the time the guardian is looking at on the Now card, so
 * that time wins. Without it, a child who has spent their Free budget at 17:30
 * would be offered "Learning time" just because Learning ended at 12:00.
 *
 * Windows naming a time the child has no definition for are skipped — minutes
 * of a time that does not exist enact nothing.
 */
export function defaultExtendTime(
  schedule: Schedule | undefined,
  times: readonly NamedTime[] | undefined,
  nowSecs: number,
): string | undefined {
  const now = activeTimeAt(schedule, times, nowSecs);
  if (now.state === "time") return now.time.id;
  if (now.state === "free") return undefined;
  // Locked ⇒ schedule is defined and not lifted.
  const sched = schedule as Schedule;
  const { dayKey, secsIntoDay } = localParts(nowSecs, sched.tz);
  const usable = windowsOn(sched, dayKey).filter((w) => isKnown(w, times ?? []));
  let ended: ScheduleWindow | undefined;
  for (const w of usable) {
    if (minutesOf(w.end) * 60 > secsIntoDay) continue;
    if (!ended || minutesOf(w.end) > minutesOf(ended.end)) ended = w;
  }
  if (ended) return ended.time;
  let next: ScheduleWindow | undefined;
  for (const w of usable) {
    if (minutesOf(w.start) * 60 <= secsIntoDay) continue;
    if (!next || minutesOf(w.start) < minutesOf(next.start)) next = w;
  }
  return next?.time;
}

/** The next time the schedule opens: for "Locked until Learning time at 09:00". */
export interface NextWindow {
  /** The named-time id that opens; absent = Free time. */
  time?: string;
  /** Unix seconds the window opens. */
  startsAt: number;
  /** The opening time on the child's clock, "HH:MM". */
  hhmm: string;
  /** "tomorrow" only when nothing is left today. */
  day: "today" | "tomorrow";
}

/**
 * The next window to open: the first one still to start today, else tomorrow's
 * first, else `null` ("Locked today" — the contract stops at tomorrow). Where a
 * tagged and an untagged window open together, the tagged one is the time that
 * runs (it carves). `null` too for an absent or lifted schedule, which never
 * locks. Pass `times` to skip windows whose tag the child has not defined: they
 * open nothing.
 */
export function nextWindow(
  schedule: Schedule | undefined,
  nowSecs: number,
  times?: readonly NamedTime[],
): NextWindow | null {
  if (!schedule || schedule.paused) return null;
  const { dayKey, secsIntoDay } = localParts(nowSecs, schedule.tz);
  const pick = (windows: ScheduleWindow[], after: number): ScheduleWindow | undefined => {
    let best: ScheduleWindow | undefined;
    for (const w of windows) {
      if (times && !isKnown(w, times)) continue;
      if (minutesOf(w.start) * 60 <= after) continue;
      if (
        !best ||
        minutesOf(w.start) < minutesOf(best.start) ||
        (minutesOf(w.start) === minutesOf(best.start) && w.time !== undefined && best.time === undefined)
      ) {
        best = w;
      }
    }
    return best;
  };
  const today = pick(windowsOn(schedule, dayKey), secsIntoDay);
  if (today) {
    const startsAt = unixAtLocal(startOfDayUnix(nowSecs, schedule.tz), minutesOf(today.start) * 60, schedule.tz);
    return { ...(today.time !== undefined ? { time: today.time } : {}), startsAt, hhmm: today.start, day: "today" };
  }
  const tomorrowStart = endOfDayUnix(nowSecs, schedule.tz);
  const tomorrowKey = localParts(tomorrowStart, schedule.tz).dayKey;
  const tomorrow = pick(windowsOn(schedule, tomorrowKey), -1);
  if (!tomorrow) return null;
  return {
    ...(tomorrow.time !== undefined ? { time: tomorrow.time } : {}),
    startsAt: unixAtLocal(tomorrowStart, minutesOf(tomorrow.start) * 60, schedule.tz),
    hhmm: tomorrow.start,
    day: "tomorrow",
  };
}

// ---------------------------------------------------------------------------
// Overlap: tagged ∩ tagged is refused; tagged ∩ untagged is a carve
// ---------------------------------------------------------------------------

/** Two tagged windows that overlap on one day. */
export interface TaggedOverlap {
  /** A weekday ("mon") or an override date ("2026-12-25"). */
  day: string;
  a: ScheduleWindow;
  b: ScheduleWindow;
}

/**
 * Every pair of TAGGED windows that overlap on one day (weekly days and each
 * override date), for any two tags — the same tag twice included, since that is
 * a window that should have been merged. A tagged window over an UNTAGGED one
 * is deliberately fine: that is how a homework hour carves Free time. Touching
 * windows (09:00–10:00 then 10:00–11:00) do not overlap.
 */
export function findTaggedOverlaps(schedule: Schedule | undefined): TaggedOverlap[] {
  if (!schedule) return [];
  const out: TaggedOverlap[] = [];
  const check = (day: string, windows: ScheduleWindow[] | undefined) => {
    const tagged = (windows ?? []).filter((w) => w.time !== undefined);
    for (let i = 0; i < tagged.length; i += 1) {
      for (let j = i + 1; j < tagged.length; j += 1) {
        const a = tagged[i];
        const b = tagged[j];
        if (a.start < b.end && b.start < a.end) out.push({ day, a, b });
      }
    }
  };
  for (const day of WEEKDAYS) check(day, schedule.weekly[day]);
  for (const [date, windows] of Object.entries(schedule.overrides ?? {})) check(date, windows);
  return out;
}

// ---------------------------------------------------------------------------
// The gate: which devices may be given tagged windows or typed gifts
// ---------------------------------------------------------------------------

/**
 * May this device be sent a tagged window or a typed gift? The same
 * `wardenSupport` answer as every other additive feature: a device that
 * REPORTS a Kintrinsic older than Android 49 / charterd 800 is out; one that
 * has not reported yet is assumed capable ("silence is not incapacity").
 *
 * That is weaker than the design's "a tagged window never reaches an ungated
 * device" for a device that has simply never reported: an old ward would run the
 * window as Free time. It is the house doctrine for every version gate, and a
 * paired ward reports within minutes; pass a stricter check upstream if a
 * caller ever needs one.
 */
export function canUseTimes(
  device: Device,
  deviceStatus: Record<string, DeviceStatus> = {},
): boolean {
  return wardenSupport([device], deviceStatus, "times").canSend;
}

// ---------------------------------------------------------------------------
// Editing the set
// ---------------------------------------------------------------------------

/**
 * What stops a time being SAVED, as sentences for the editor (empty = fine).
 * Everything the device would refuse, plus what the device allows but the
 * guardian must not save: an *only these* time with no apps and no sites runs
 * nothing, and two sites sharing an id would collide on one launcher.
 */
export function namedTimeProblems(time: NamedTime): string[] {
  const out: string[] = [];
  if (typeof time.label !== "string" || time.label.trim() === "") {
    out.push("Give this time a name.");
  } else {
    const fault = namedTimeFault({ ...time, label: time.label.trim() });
    if (fault) out.push(fault);
  }
  if (time.posture === "onlyThese" && !(time.apps?.length || time.sites?.length)) {
    out.push("Choose at least one app or website for this time.");
  }
  const ids = (time.posture === "onlyThese" ? time.sites : undefined)?.map((s) => s.id) ?? [];
  if (new Set(ids).size !== ids.length) out.push("The same website is listed twice.");
  return out;
}

/** Add a time. Throws if the set is full, the id is taken, or it is invalid. */
export function addNamedTime(policy: Policy, time: NamedTime): Policy {
  const existing = policy.times ?? [];
  if (existing.length >= MAX_NAMED_TIMES) {
    throw new Error(`A child can have at most ${MAX_NAMED_TIMES} times.`);
  }
  if (existing.some((t) => t.id === time.id)) throw new Error(`There is already a time with the id "${time.id}".`);
  const fault = namedTimeFault({ ...time, label: time.label.trim() });
  if (fault) throw new Error(fault);
  return { ...policy, times: [...existing, time] };
}

/** Replace a time's definition (same `id`). Throws if there is no such time. */
export function updateNamedTime(policy: Policy, time: NamedTime): Policy {
  const existing = policy.times ?? [];
  if (!existing.some((t) => t.id === time.id)) throw new Error(`There is no time "${time.id}".`);
  const fault = namedTimeFault({ ...time, label: time.label.trim() });
  if (fault) throw new Error(fault);
  return { ...policy, times: existing.map((t) => (t.id === time.id ? time : t)) };
}

/**
 * Rename a time. Changes `label` and NOTHING else: the `id` is what every
 * tagged window, pool and gift refers to, so a rename that touched it would
 * strand the windows (they would lock) and orphan today's pools.
 */
export function renameNamedTime(policy: Policy, id: string, label: string): Policy {
  const trimmed = label.trim();
  if (trimmed === "" || Array.from(trimmed).length > MAX_TIME_LABEL_LEN) {
    throw new Error(`A time's name must be 1-${MAX_TIME_LABEL_LEN} characters.`);
  }
  const existing = policy.times ?? [];
  if (!existing.some((t) => t.id === id)) throw new Error(`There is no time "${id}".`);
  return { ...policy, times: existing.map((t) => (t.id === id ? { ...t, label: trimmed } : t)) };
}

// ---------------------------------------------------------------------------
// Windows: stripping a time's tags, writing them per device
// ---------------------------------------------------------------------------

/** A schedule with every window tagged `id` removed (`undefined` in, out). A
 *  weekday that ends up empty is dropped; an EMPTY OVERRIDE is kept, because an
 *  override with no windows is a closed day and deleting it would hand that
 *  date back to the weekly pattern — a loosening nobody asked for. */
function stripTag(schedule: Schedule | undefined, id: string): Schedule | undefined {
  if (!schedule) return schedule;
  const has = (ws: ScheduleWindow[] | undefined) => ws?.some((w) => w.time === id) ?? false;
  const anyTagged =
    WEEKDAYS.some((d) => has(schedule.weekly[d])) ||
    Object.values(schedule.overrides ?? {}).some((ws) => has(ws));
  if (!anyTagged) return schedule;
  const weekly: Schedule["weekly"] = {};
  for (const day of WEEKDAYS) {
    const ws = schedule.weekly[day];
    if (!ws) continue;
    const kept = ws.filter((w) => w.time !== id);
    if (kept.length > 0) weekly[day] = kept;
    else if (!has(ws)) weekly[day] = ws; // an untouched empty day stays as it was
  }
  const next: Schedule = { ...schedule, weekly };
  if (schedule.overrides) {
    next.overrides = Object.fromEntries(
      Object.entries(schedule.overrides).map(([date, ws]) => [date, ws.filter((w) => w.time !== id)]),
    );
  }
  return next;
}

/**
 * Delete a named time AND every window tagged with it — in the base schedule
 * and in every device's own schedule — so one signed save leaves no device
 * locked on an id that no longer exists. Free time cannot be deleted.
 *
 * The caller must sign the `times` and `schedule` dimensions together (the
 * result changes both, and `deviceOverrides` for a split schedule). Deleting
 * the last time leaves `times: []`, which signs a real empty clause.
 */
export function removeNamedTime(policy: Policy, id: string): Policy {
  if (id === FREE_TIME_ID) throw new Error("Free time can't be deleted.");
  const next: Policy = { ...policy };
  if (policy.times) next.times = policy.times.filter((t) => t.id !== id);
  const schedule = stripTag(policy.schedule, id);
  if (schedule !== policy.schedule) next.schedule = schedule;
  if (policy.deviceOverrides) {
    const overrides: NonNullable<Policy["deviceOverrides"]> = {};
    for (const [deviceId, o] of Object.entries(policy.deviceOverrides)) {
      const s = stripTag(o.schedule, id);
      overrides[deviceId] = s === o.schedule ? o : { ...o, schedule: s };
    }
    next.deviceOverrides = overrides;
  }
  return next;
}

/** One time's windows on one device: the weekly pattern, as plain windows (the
 *  tag is applied by `setTimeWindows`, so a caller cannot get it wrong). */
export type TimeWindows = Partial<Record<Weekday, Array<{ start: string; end: string }>>>;

/**
 * Write a named time's weekly windows into ONE device's schedule, replacing the
 * windows already tagged with that time there (date overrides are left alone;
 * untagged and other-tagged windows are never touched).
 *
 * Three guards, each a way a window could reach a device it must not:
 *  - the time must be one the child defines (else the span would lock);
 *  - the device must pass the `times` gate (`canUseTimes`) — a pre-times ward
 *    would run the window as Free time;
 *  - the device must hold its OWN schedule first. If the schedule is still
 *    shared, it is split (`setControlSplit`, seeding EVERY device with the
 *    shared value) so the siblings keep an untagged schedule; and if it is
 *    already split but this device follows the base, it is split the same way,
 *    since writing into the base would reach the siblings too.
 *
 * Throws (with a sentence a guardian can read) rather than returning a policy
 * that would put a tagged window somewhere unsafe, or two tagged windows on top
 * of each other. `devices` is every device of the child; `deviceStatus` is the
 * live STATUS by device pubkey, as `wardenSupport` takes it.
 */
export function setTimeWindows(
  policy: Policy,
  id: string,
  deviceId: string,
  windows: TimeWindows,
  devices: Device[],
  deviceStatus: Record<string, DeviceStatus> = {},
): Policy {
  if (!isValidTimeId(id) || !(policy.times ?? []).some((t) => t.id === id)) {
    throw new Error(`There is no time "${id}" to give windows to.`);
  }
  const device = devices.find((d) => d.id === deviceId);
  if (!device) throw new Error("That device isn't one of this child's.");
  if (!canUseTimes(device, deviceStatus)) {
    throw new Error(`${device.label} needs the latest Kintrinsic before it can run a time.`);
  }
  for (const day of WEEKDAYS) {
    for (const w of windows[day] ?? []) {
      if (!HHMM.test(w.start) || !HHMM.test(w.end) || w.start >= w.end) {
        throw new Error(`A window must start before it ends (got ${w.start}-${w.end}).`);
      }
    }
  }

  let next = policy;
  if (next.deviceOverrides?.[deviceId]?.schedule === undefined) {
    if (!next.schedule) {
      throw new Error("Set up a schedule for this child before adding a time to it.");
    }
    next = setControlSplit(next, "schedule", devices.map((d) => d.id));
  }
  const own = next.deviceOverrides?.[deviceId]?.schedule as Schedule;
  if (own.paused) {
    throw new Error(`${device.label}'s schedule is switched off, so a time would have no effect. Switch it on first.`);
  }

  const without = stripWeeklyTag(own, id);
  const weekly: Schedule["weekly"] = { ...without.weekly };
  for (const day of WEEKDAYS) {
    const add = windows[day] ?? [];
    if (add.length === 0) continue;
    weekly[day] = [...(weekly[day] ?? []), ...add.map((w) => ({ start: w.start, end: w.end, time: id }))];
  }
  const schedule: Schedule = { ...without, weekly };
  const overlap = findTaggedOverlaps(schedule)[0];
  if (overlap) {
    throw new Error(
      `Two times overlap on ${overlap.day} (${overlap.a.start}-${overlap.a.end} and ${overlap.b.start}-${overlap.b.end}). Move one so they don't run at the same time.`,
    );
  }
  return setDeviceControl(next, deviceId, "schedule", schedule);
}

/** `stripTag` for the weekly pattern only (date overrides keep their tags). */
function stripWeeklyTag(schedule: Schedule, id: string): Schedule {
  const weekly: Schedule["weekly"] = {};
  for (const day of WEEKDAYS) {
    const ws = schedule.weekly[day];
    if (!ws) continue;
    const kept = ws.filter((w) => w.time !== id);
    if (kept.length > 0 || ws.length === 0) weekly[day] = kept;
  }
  return { ...schedule, weekly };
}

/** Time ids that some schedule (base or per device) tags but the child does not
 *  define. Each such span is LOCKED on the device; the editor should say so. */
export function unknownTimeTags(policy: Policy): string[] {
  const defined = new Set((policy.times ?? []).map((t) => t.id));
  const seen = new Set<string>();
  const scan = (s: Schedule | undefined) => {
    if (!s) return;
    const all = [...WEEKDAYS.flatMap((d) => s.weekly[d] ?? []), ...Object.values(s.overrides ?? {}).flat()];
    for (const w of all) if (w.time !== undefined && !defined.has(w.time)) seen.add(w.time);
  };
  scan(policy.schedule);
  for (const o of Object.values(policy.deviceOverrides ?? {})) scan(o.schedule);
  return [...seen];
}

// ---------------------------------------------------------------------------
// Templates for "Add a time"
// ---------------------------------------------------------------------------

/**
 * The pre-made Learning time: *only these*, no apps, no windows, and — the
 * point of it — `countsAgainstBudget: false`, so homework never costs the
 * child's leisure budget. The id is fixed (`learning`); the caller must not
 * offer the template when the child already has that id.
 *
 * `sites` seeds the websites (the design pre-seeds them from the learning
 * catalogue picks); the default is none, so an editor that forgets to seed
 * gets a time that runs nothing and refuses to save rather than one that
 * silently opens something.
 */
export function learningTimeTemplate(sites: LearningApp[] = []): NamedTime {
  return {
    id: "learning",
    label: "Learning time",
    posture: "onlyThese",
    apps: [],
    sites,
    countsAgainstBudget: false,
  };
}

/** A `[a-z0-9-]{1,32}` id for a typed name, unique against `taken` and never
 *  the reserved `free`. Never leads or trails with a hyphen. */
export function timeSlug(label: string, taken: readonly string[] = []): string {
  const base =
    label
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 32)
      .replace(/-+$/g, "") || "time";
  const used = (id: string) => id === FREE_TIME_ID || taken.includes(id);
  if (!used(base)) return base;
  for (let n = 2; ; n += 1) {
    const suffix = `-${n}`;
    const id = `${base.slice(0, 32 - suffix.length).replace(/-+$/g, "")}${suffix}`;
    if (!used(id)) return id;
  }
}

/**
 * A blank custom time named by the guardian. *Only these* with nothing listed:
 * the restrictive default, so a time that is saved before it is finished cannot
 * be looser than the guardian meant (the editor refuses to save it empty — see
 * `namedTimeProblems`). The id is slugged from the label and de-duplicated
 * against `takenIds`; it never changes after this, whatever the label becomes.
 */
export function customTimeTemplate(label: string, takenIds: readonly string[] = []): NamedTime {
  return {
    id: timeSlug(label, takenIds),
    label: Array.from(label.trim()).slice(0, MAX_TIME_LABEL_LEN).join(""),
    posture: "onlyThese",
    apps: [],
    sites: [],
    countsAgainstBudget: false,
  };
}
