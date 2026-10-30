// What the Times screens say, as pure functions — so every sentence a guardian
// reads about a time (its hours, what it allows, who is left with nothing) is
// testable without mounting anything.
//
// A *time* here is always a window with a posture (Free time, Learning time…).
// The older "named times" (groups of apps inside Free time) are a different
// thing and live in `screens/NamedTimes.tsx`.

import { effectivePolicyForDevice } from "../domain/effectivePolicy";
import type { TimeWindows } from "../domain/times";
import { WEEKDAYS, type Child, type Device, type NamedTime, type Policy, type Schedule, type Weekday } from "../domain/types";

const DAY_SHORT: Record<Weekday, string> = {
  mon: "Mon",
  tue: "Tue",
  wed: "Wed",
  thu: "Thu",
  fri: "Fri",
  sat: "Sat",
  sun: "Sun",
};

/** One named time's weekly windows on a schedule, as plain (untagged) windows. */
export function weeklyWindowsOf(schedule: Schedule | undefined, id: string): TimeWindows {
  const out: TimeWindows = {};
  if (!schedule) return out;
  for (const day of WEEKDAYS) {
    const mine = (schedule.weekly[day] ?? []).filter((w) => w.time === id);
    if (mine.length > 0) out[day] = mine.map((w) => ({ start: w.start, end: w.end }));
  }
  return out;
}

/** The windows a time has on ONE device, from that device's effective schedule
 *  (its own if it has one, else the shared one). */
export function windowsOnDevice(policy: Policy, id: string, deviceId: string): TimeWindows {
  return weeklyWindowsOf(effectivePolicyForDevice(policy, deviceId).schedule, id);
}

/** True when a device's schedule has no windows at all in the week: the wire
 *  then emits "paused", which locks the device all day. */
function hasNoWindows(schedule: Schedule | undefined): boolean {
  if (!schedule || schedule.paused) return false;
  if (schedule.overrides && Object.values(schedule.overrides).some((ws) => ws.length > 0)) return false;
  return WEEKDAYS.every((d) => (schedule.weekly[d] ?? []).length === 0);
}

/**
 * The devices that would have NO allowed time at all after this policy is
 * saved. Deleting the only time a laptop ran (with no Free hours on it) leaves
 * it with an empty schedule, which the device reads as "locked all day" — worth
 * saying before the guardian does it, not after.
 */
export function devicesLeftWithoutHours(after: Policy, devices: readonly Device[]): Device[] {
  return devices.filter(
    (d) => d.pairing === "paired" && hasNoWindows(effectivePolicyForDevice(after, d.id).schedule),
  );
}

const hhmm = (w: { start: string; end: string }) => `${w.start}–${w.end}`;

/** "Mon–Fri 09:00–11:00 · Sat 10:00–12:00", runs of days with the same hours
 *  folded together; "No hours yet" when empty. */
export function windowsSummary(w: TimeWindows): string {
  const key = (d: Weekday) => (w[d] ?? []).map(hhmm).join(", ");
  const parts: string[] = [];
  let i = 0;
  while (i < WEEKDAYS.length) {
    const k = key(WEEKDAYS[i]);
    if (!k) {
      i += 1;
      continue;
    }
    let j = i;
    while (j + 1 < WEEKDAYS.length && key(WEEKDAYS[j + 1]) === k) j += 1;
    const span = j - i + 1;
    const days =
      span === 7
        ? "Every day"
        : span === 1
          ? DAY_SHORT[WEEKDAYS[i]]
          : span === 2
            ? `${DAY_SHORT[WEEKDAYS[i]]}, ${DAY_SHORT[WEEKDAYS[j]]}`
            : `${DAY_SHORT[WEEKDAYS[i]]}–${DAY_SHORT[WEEKDAYS[j]]}`;
    parts.push(`${days} ${k}`);
    i = j + 1;
  }
  return parts.length > 0 ? parts.join(" · ") : "No hours yet";
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** One line on what a time allows, for its card. */
export function postureSummary(t: NamedTime): string {
  if (t.posture === "onlyThese") {
    const sites = t.sites?.length ?? 0;
    const apps = t.apps?.length ?? 0;
    if (sites + apps === 0) return "Only these — nothing chosen yet";
    const bits: string[] = [];
    if (sites) bits.push(sites <= 2 ? (t.sites ?? []).map((s) => s.label).join(", ") : plural(sites, "website", "websites"));
    if (apps) bits.push(plural(apps, "app", "apps"));
    return `Only ${bits.join(" and ")}`;
  }
  const changes =
    (t.blockApps?.length ?? 0) +
    (t.unblockApps?.length ?? 0) +
    (t.denySites?.length ?? 0) +
    (t.undenySites?.length ?? 0);
  return changes === 0
    ? "Everything Free time allows"
    : `Everything Free time allows, with ${plural(changes, "change", "changes")}`;
}

/**
 * The line under a time's name on its card: what it allows, and whether it
 * costs. Hours are per device and shown beside each device, not here.
 */
export function timeCardSummary(t: NamedTime): string {
  return `${postureSummary(t)} · ${t.countsAgainstBudget ? "Uses up screen time" : "Doesn’t use up screen time"}`;
}

/** The title for the header: the time's name, or what is about to be made. */
export function timeEditorTitle(
  child: Child,
  r: { timeId?: string; template?: "learning" | "custom"; name?: string },
): string {
  if (r.timeId) {
    const policy = child.policies.find((p) => p.scope.kind === "device");
    return policy?.times?.find((t) => t.id === r.timeId)?.label ?? "Time";
  }
  if (r.template === "learning") return "Learning time";
  return r.name?.trim() || "New time";
}

/** Does any weekday of this schedule carry a window for a named time? */
export function hasTaggedWindows(schedule: Schedule | undefined): boolean {
  return (
    !!schedule && WEEKDAYS.some((d) => (schedule.weekly[d] ?? []).some((w) => w.time !== undefined))
  );
}
