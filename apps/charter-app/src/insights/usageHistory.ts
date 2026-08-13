// Per-device, per-day screen-time history + the weekly view (design memo B1).
//
// Data source today: the device STATUS heartbeats Kintrinsic already receives.
// `usedTodaySecs` is cumulative and monotonic within a day, so we keep the MAX
// observed per device per day. This is *live-derived* — a day the app never
// polled can undercount — which the device-journaled usage-sync (kind 31115)
// backfill closes later. The SHAPE below is identical either way, so none of
// this is throwaway.
//
// Companion frame (see docs/CONSTITUTION.md): this is REFLECTION, not policing.
// Going over the agreed time is an overdraft to notice and wind back, shown as
// the red part of the week — never a slammed door. No scores, no streaks.

import {
  decodeMinutes,
  emptyMinutes,
  encodeMinutes,
  minuteCount,
  unionMinutes,
} from "./minuteSet";

export interface UsageHistory {
  /** machine pubkey -> "YYYY-MM-DD" (device tz) -> seconds used that day. */
  byMachine: Record<string, Record<string, number>>;
  /** machine pubkey -> dayKey -> MinuteSet b64url (240 chars) — the device's
   *  active-minutes journal (B3). Parallel to `byMachine`; merged by UNION
   *  across polls. Optional so pre-B3 persisted histories load unchanged. */
  minutesByMachine?: Record<string, Record<string, string>>;
}

export const emptyUsageHistory: UsageHistory = { byMachine: {} };

const DAY_KEY = /^\d{4}-\d{2}-\d{2}$/;
const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/**
 * Record a device's cumulative same-day usage, keeping the max seen (usage is
 * monotonic within a day) and merging the optional minute journal by UNION.
 * Returns the SAME reference when nothing changed, so repeat polls are a
 * cheap no-op and don't churn React state.
 */
export function recordUsage(
  h: UsageHistory,
  machine: string,
  dayKey: string,
  usedSecs: number,
  minutesB64?: string,
): UsageHistory {
  if (!machine || !DAY_KEY.test(dayKey) || !Number.isFinite(usedSecs) || usedSecs < 0) return h;

  const days = h.byMachine[machine] ?? {};
  const secsChanged = usedSecs > (days[dayKey] ?? 0);

  // Merge the incoming journal (already strict-validated at parseStatus) by
  // union with anything previously seen for that (machine, day).
  let mergedMinutes: string | undefined;
  const prevMinutes = h.minutesByMachine?.[machine]?.[dayKey];
  if (minutesB64) {
    const incoming = decodeMinutes(minutesB64);
    if (incoming) {
      const prev = prevMinutes ? decodeMinutes(prevMinutes) : null;
      const merged = prev ? encodeMinutes(unionMinutes(prev, incoming)) : minutesB64;
      if (merged !== prevMinutes) mergedMinutes = merged;
    }
  }

  if (!secsChanged && mergedMinutes === undefined) return h;

  const out: UsageHistory = {
    byMachine: secsChanged
      ? { ...h.byMachine, [machine]: { ...days, [dayKey]: Math.round(usedSecs) } }
      : h.byMachine,
    minutesByMachine: h.minutesByMachine,
  };
  if (mergedMinutes !== undefined) {
    out.minutesByMachine = {
      ...(h.minutesByMachine ?? {}),
      [machine]: { ...(h.minutesByMachine?.[machine] ?? {}), [dayKey]: mergedMinutes },
    };
  }
  return out;
}

/** Drop days older than `keepDays` so the store can't grow forever. */
export function pruneHistory(h: UsageHistory, nowMs: number, keepDays = 60): UsageHistory {
  const cutoff = localDayKey(new Date(nowMs - keepDays * 86_400_000));
  let changed = false;
  const byMachine: UsageHistory["byMachine"] = {};
  for (const [m, days] of Object.entries(h.byMachine)) {
    const kept: Record<string, number> = {};
    for (const [k, v] of Object.entries(days)) {
      if (k >= cutoff) kept[k] = v;
      else changed = true;
    }
    byMachine[m] = kept;
  }
  let minutesByMachine = h.minutesByMachine;
  if (minutesByMachine) {
    const keptAll: Record<string, Record<string, string>> = {};
    for (const [m, days] of Object.entries(minutesByMachine)) {
      const kept: Record<string, string> = {};
      for (const [k, v] of Object.entries(days)) {
        if (k >= cutoff) kept[k] = v;
        else changed = true;
      }
      keptAll[m] = kept;
    }
    minutesByMachine = keptAll;
  }
  return changed ? { byMachine, minutesByMachine } : h;
}

export interface DayUsage {
  dayKey: string; // YYYY-MM-DD (local)
  label: string; // "Mon"
  /** Union of active minutes ×60 when every using device journaled minutes
   *  that day (simultaneous use counts once); scalar sum otherwise. */
  totalSecs: number;
  perDevice: { machine: string; secs: number }[]; // in the given machine order
  /** Time on 2+ devices at once: (Σ per-device minutes − union) ×60. Zero on
   *  scalar-fallback days. */
  overlapSecs: number;
  allowanceSecs: number; // agreed daily budget (0 = none set)
  overdraftSecs: number; // max(0, total - allowance) when an allowance is set
}

/**
 * The last 7 local days ending today, for one child's devices. `nowMs` and the
 * machine list are injected for testability. (Day keys are matched against the
 * device's tz key; for the common same-tz case they align — a cross-tz refinement
 * is deferred with the usage-sync backfill.)
 */
export function weeklyView(
  h: UsageHistory,
  machines: string[],
  allowanceMinutes: number | null | undefined,
  nowMs: number,
): DayUsage[] {
  const allowanceSecs = allowanceMinutes && allowanceMinutes > 0 ? allowanceMinutes * 60 : 0;
  const out: DayUsage[] = [];
  for (let i = 6; i >= 0; i--) {
    const d = new Date(nowMs);
    d.setDate(d.getDate() - i);
    const dayKey = localDayKey(d);
    const perDevice = machines.map((m) => ({ machine: m, secs: h.byMachine[m]?.[dayKey] ?? 0 }));
    const scalarSum = perDevice.reduce((s, p) => s + p.secs, 0);

    // Union rule: only when EVERY device with usage that day journaled
    // minutes — a mix would undercount the journal-less device's share.
    const using = perDevice.filter((p) => p.secs > 0);
    const journals = using
      .map((p) => h.minutesByMachine?.[p.machine]?.[dayKey])
      .map((s) => (s ? decodeMinutes(s) : null));
    let totalSecs = scalarSum;
    let overlapSecs = 0;
    if (using.length > 0 && journals.every((j) => j !== null)) {
      const bitmaps = journals as Uint8Array[];
      const union = bitmaps.reduce((acc, j) => unionMinutes(acc, j), emptyMinutes());
      const perDeviceMinutes = bitmaps.reduce((s, j) => s + minuteCount(j), 0);
      totalSecs = minuteCount(union) * 60;
      overlapSecs = Math.max(0, perDeviceMinutes * 60 - totalSecs);
    }

    out.push({
      dayKey,
      label: WEEKDAYS[d.getDay()],
      totalSecs,
      perDevice,
      overlapSecs,
      allowanceSecs,
      overdraftSecs: allowanceSecs > 0 ? Math.max(0, totalSecs - allowanceSecs) : 0,
    });
  }
  return out;
}

export function localDayKey(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

export function humanDuration(secs: number): string {
  const m = Math.round(secs / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  return rem ? `${h}h ${rem}m` : `${h}h`;
}

/** A calm, reflective one-liner for the week — no scores, no praise/blame. */
export function weekSummary(days: DayUsage[]): string {
  const total = days.reduce((s, d) => s + d.totalSecs, 0);
  if (total === 0) return "No screen time recorded yet this week.";
  const over = days.filter((d) => d.overdraftSecs > 0).length;
  const parts = [`This week: ${humanDuration(total)}`];
  if (over > 0) parts.push(`${over} day${over > 1 ? "s" : ""} over`);
  return parts.join(" · ");
}

/**
 * The week's out-of-hours use, in the same calm register as [weekSummary] —
 * a fact about the week, never a warning. `null` when there is nothing to
 * say, so a family that never set the clause sees no line at all. Uses the
 * SAME formatter as the ward's own mirror (`GroupMirror.outOfHoursLine`,
 * Android) — the family agreed both of them see this plainly, in the same
 * words — though each side feeds it its OWN device's numbers; see that
 * function's own doc for what "identical" does and doesn't guarantee.
 *
 * "so far this week", not just "this week" (review fix I2, 2026-08-04): this
 * line resets on a CALENDAR week (`week_start`, the device's own tz) while
 * `weekSummary` right beside it is a ROLLING 7 days ending today, in the
 * guardian's tz — two different windows that would otherwise both read as
 * plain "this week" in the same card. Late Monday, three out-of-hours nights
 * from the week just closed would silently vanish (the calendar week having
 * just reset) while the chart above still shows those bars — read as "the
 * night use stopped" rather than "the counter rolled over". The scope word
 * doesn't fix the reset, but it stops the line from claiming a window it
 * isn't measuring.
 */
export function outOfHoursLine(nights: number, secs: number): string | null {
  if (nights === 0 || secs === 0) return null;
  return `${nights} night${nights > 1 ? "s" : ""} so far this week · ${humanDuration(secs)}`;
}

/**
 * The calm placeholder for a week with zero COUNTED screen time — aware of
 * whether there was real out-of-hours use, so it can never contradict an
 * `outOfHoursLine` printed right below it (review fix C1, 2026-08-04).
 *
 * Before this, `WeeklyPicture` always printed "No screen time recorded yet
 * this week…" whenever `usedTodaySecs`-derived totals were zero — which they
 * always are for a ward whose ONLY device use is an always-available app
 * during locked hours, because `outOfHoursTodaySecs` is deliberately absent
 * from `usedTodaySecs` (it must never touch the budget). That ward is
 * EXACTLY who this feature is for, so the empty state and the out-of-hours
 * line disagreed in the one case the feature exists to describe honestly.
 *
 * `hasOutOfHours` should be `outOfHoursLine(...) != null` for the SAME week —
 * the caller is responsible for that agreement (see `WeeklyPicture`, which is
 * the sole caller and always passes the two together).
 */
export function emptyWeekMessage(hasOutOfHours: boolean): string {
  return hasOutOfHours
    ? "No counted screen time this week."
    : "No screen time recorded yet this week — it fills in as the devices are used.";
}
