// "Same every day" for the schedule editor.
//
// decented, 2026-07-27: setting allowed hours meant filling in the same times
// seven times over, when almost every family starts from one rule for the whole
// week and only then carves out a different Saturday.
//
// Purely an editing affordance: the wire and the device are unchanged, because
// `weekly` already stores all seven days. "Same every day" writes the same
// windows to each one — so nothing downstream has to learn a new shape, and a
// charter authored this way is byte-identical to one typed out day by day.

import { WEEKDAYS, type Schedule, type ScheduleWindow, type Weekday } from "./types";

/** The windows for a day, normalised (absent == blocked == no windows). */
function windowsOf(schedule: Schedule, day: Weekday): ScheduleWindow[] {
  return schedule.weekly[day] ?? [];
}

/** Stable comparison key for a day's windows — order is meaningful, so no sort. */
function keyOf(windows: ScheduleWindow[]): string {
  return windows.map((w) => `${w.start}-${w.end}`).join("|");
}

/**
 * Whether every day of the week currently says the same thing.
 *
 * This is what the editor opens on, rather than a stored preference: a charter
 * that IS the same every day should open in the simple view, and one that
 * genuinely differs must never open in a view that cannot show the difference.
 */
export function isSameEveryDay(schedule: Schedule): boolean {
  const keys = WEEKDAYS.map((d) => keyOf(windowsOf(schedule, d)));
  return new Set(keys).size === 1;
}

/**
 * The day whose times become the shared ones when collapsing to "same every
 * day": the first day of the week that actually allows something.
 *
 * Deliberately not "Monday, always" — collapsing a weekend-only charter onto a
 * blocked Monday would silently turn the whole week off. Falls back to Monday
 * when nothing is allowed anywhere, where there is nothing to lose either way.
 */
export function collapseSourceDay(schedule: Schedule): Weekday {
  return WEEKDAYS.find((d) => windowsOf(schedule, d).length > 0) ?? WEEKDAYS[0];
}

/**
 * Apply one day's windows to the whole week.
 *
 * Lossy by nature — that is exactly why the editor asks first whenever
 * [`isSameEveryDay`] is false. Cloned per day so later per-day edits can never
 * mutate a sibling through a shared reference.
 */
export function applyToEveryDay(
  schedule: Schedule,
  windows: ScheduleWindow[],
): Schedule {
  const weekly = { ...schedule.weekly };
  for (const d of WEEKDAYS) {
    weekly[d] = windows.map((w) => ({ ...w }));
  }
  return { ...schedule, weekly };
}

/** The windows the "same every day" view edits: whatever the source day says. */
export function sharedWindows(schedule: Schedule): ScheduleWindow[] {
  return windowsOf(schedule, collapseSourceDay(schedule)).map((w) => ({ ...w }));
}
