import type { Child, Schedule, ScheduleWindow, Weekday } from "../domain/types";

/**
 * isSetUp — a child is "set up" once at least one of their devices is paired.
 * Every "Device connected" / "Not set up" check reads this (it replaced the
 * old `child.devicePaired` flag).
 */
export function isSetUp(child: Child): boolean {
  return child.devices.some((d) => d.pairing === "paired");
}

/**
 * statusFor — the one pure selector every screen uses to answer
 * "can this child use their device right now, and for how long?".
 *
 * Effective time = min(time left in today's schedule window, budget left today).
 *
 * NOTE: usage tracking (minutes actually consumed today) is a later phase, so
 * budget-left currently equals the full daily allowance. The shape and the
 * min() composition are final; only the "used" input is stubbed.
 */
export interface StatusForResult {
  allowedNow: boolean;
  /** `standdown` only ever arrives from a device's REAL report (liveStatus);
   *  the local guess can't know a guardian called one. */
  reason?: "schedule" | "budget" | "standdown";
  /** Effective minutes left today; null means no cap applies. 0 when locked. */
  minutesLeftToday: number | null;
  /** Friendly "Opens at 4:00 PM" text when schedule-locked. */
  nextWindowText?: string;
}

const WEEKDAY_BY_INDEX: Weekday[] = [
  "sun",
  "mon",
  "tue",
  "wed",
  "thu",
  "fri",
  "sat",
];

const WEEKDAY_LABEL: Record<Weekday, string> = {
  mon: "Mon",
  tue: "Tue",
  wed: "Wed",
  thu: "Thu",
  fri: "Fri",
  sat: "Sat",
  sun: "Sun",
};

function toMinutes(hhmm: string): number {
  const [h, m] = hhmm.split(":").map((n) => parseInt(n, 10));
  return h * 60 + m;
}

/** "16:00" -> "4:00 PM" */
export function formatClock(hhmm: string): string {
  const [h, m] = hhmm.split(":").map((n) => parseInt(n, 10));
  const period = h < 12 ? "AM" : "PM";
  const h12 = h % 12 === 0 ? 12 : h % 12;
  return `${h12}:${String(m).padStart(2, "0")} ${period}`;
}

function isoDate(d: Date): string {
  const y = d.getFullYear();
  const mo = String(d.getMonth() + 1).padStart(2, "0");
  const da = String(d.getDate()).padStart(2, "0");
  return `${y}-${mo}-${da}`;
}

/** Windows that apply on a given calendar date (overrides beat weekly). */
function windowsForDate(sched: Schedule, d: Date): ScheduleWindow[] {
  const override = sched.overrides?.[isoDate(d)];
  if (override) return override;
  const wd = WEEKDAY_BY_INDEX[d.getDay()];
  return sched.weekly[wd] ?? [];
}

function nextWindowText(sched: Schedule, base: Date, curMin: number): string | undefined {
  for (let i = 0; i < 7; i++) {
    const d = new Date(base);
    d.setDate(d.getDate() + i);
    const windows = windowsForDate(sched, d)
      .slice()
      .sort((a, b) => toMinutes(a.start) - toMinutes(b.start));
    for (const w of windows) {
      const startMin = toMinutes(w.start);
      if (i === 0 && startMin <= curMin) continue;
      const clock = formatClock(w.start);
      if (i === 0) return `Opens at ${clock}`;
      if (i === 1) return `Opens tomorrow at ${clock}`;
      return `Opens ${WEEKDAY_LABEL[WEEKDAY_BY_INDEX[d.getDay()]]} at ${clock}`;
    }
  }
  return undefined;
}

export function statusFor(child: Child, now: number): StatusForResult {
  const policy = child.policies.find((p) => p.scope.kind === "device");
  if (!policy) {
    return { allowedNow: true, minutesLeftToday: null };
  }

  const d = new Date(now);
  const curMin = d.getHours() * 60 + d.getMinutes();

  // --- Schedule ---
  let scheduleLeft = Infinity;
  let scheduleBlocked = false;
  let nextText: string | undefined;
  const sched = policy.schedule;
  if (sched && !sched.paused) {
    const windows = windowsForDate(sched, d);
    const active = windows.find(
      (w) => curMin >= toMinutes(w.start) && curMin < toMinutes(w.end),
    );
    if (active) {
      scheduleLeft = toMinutes(active.end) - curMin;
    } else {
      scheduleBlocked = true;
      scheduleLeft = 0;
      nextText = nextWindowText(sched, d, curMin);
    }
  }

  if (scheduleBlocked) {
    return {
      allowedNow: false,
      reason: "schedule",
      minutesLeftToday: 0,
      nextWindowText: nextText,
    };
  }

  // --- Budget --- (TODO: subtract real usage when tracking lands)
  let budgetLeft = Infinity;
  const bud = policy.budget;
  if (bud && !bud.paused && bud.dailyMinutes != null) {
    budgetLeft = bud.dailyMinutes;
  }

  const effective = Math.min(scheduleLeft, budgetLeft);
  if (effective <= 0) {
    return {
      allowedNow: false,
      reason: budgetLeft <= scheduleLeft ? "budget" : "schedule",
      minutesLeftToday: 0,
    };
  }

  return {
    allowedNow: true,
    minutesLeftToday: effective === Infinity ? null : Math.floor(effective),
  };
}
