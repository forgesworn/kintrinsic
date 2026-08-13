// Named times' group progress, for the guardian's "how's their day going"
// view — STATUS `groups` (raw spent seconds, day + week) joined against the
// saved policy's counted groups (label + caps). Pure and testable: STATUS
// carries no label, only the id the wire keys the meter by, and a cap axis
// should only ever show when the family actually set one.

import type { ActivityEvent, AppBucketRule, BucketsPolicy } from "./types";
import type { StatusGroup } from "../wire/status";

/** One group's progress row, ready to render. */
export interface GroupProgressRow {
  id: string;
  /** The saved bucket's label, falling back to the raw wire id when this
   *  group is in STATUS but the policy no longer (or never did) name it —
   *  a renamed/removed group, or a device on a clause this app didn't sign,
   *  still gets an honest row rather than vanishing. */
  label: string;
  /** Raw meter, seconds spent today — never subtracts a `gift`/extend, so it
   *  reads the same thing the device itself is enforcing against. */
  daySecs: number;
  /** Raw meter, seconds spent this week. */
  weekSecs: number;
  /** Present only when the saved policy caps this group's day. */
  dailyMinutes?: number;
  /** Present only when the saved policy caps this group's week. */
  weeklyMinutes?: number;
  /**
   * Today's total minutes the GUARDIAN granted to this group (extends +
   * gifts), when any — `groupExtrasToday`'s join. Present only when > 0.
   * `groupProgressLine` adds this onto the displayed cap: STATUS's raw
   * `daySecs`/`weekSecs` correctly include the extra time spent, but the
   * saved policy's `dailyMinutes`/`weeklyMinutes` is the BASE cap only, so
   * joining against it alone reads a grant the guardian personally approved
   * as a breach that never happened (M-2, hardware round 2026-08-03).
   */
  extraMinutesToday?: number;
}

/**
 * Today's total minutes the guardian granted to each group — every extend
 * (a bucket-hit `time.extend` approval) and gift (`giveTime` with a
 * `groupId`) recorded since `todayStartUnixMs`, keyed by group id. Both ride
 * the SAME structured `ActivityEvent` fields (`store.tsx`'s `addActivity`
 * call sites), so this is the one place both are summed together — there is
 * no separate "extension pool" the guardian's own app can read back from the
 * device, only what it remembers granting.
 *
 * Scoped to `childId` (activity is a shared log across the whole family) and
 * to `todayStartUnixMs` (the caller's job — see `wire/grant.ts`'s
 * `startOfDayUnix`, keyed by the GROUP's own tz, never the guardian phone's).
 * A day-scoped join means a guardian who's had this app open across a day
 * roll won't see yesterday's grant still padding out today's cap.
 */
export function groupExtrasToday(
  activity: ActivityEvent[],
  childId: string,
  todayStartUnixMs: number,
): Record<string, number> {
  const extras: Record<string, number> = {};
  for (const a of activity) {
    if (a.childId !== childId || !a.bucketId || !a.minutesGranted || a.ts < todayStartUnixMs) {
      continue;
    }
    extras[a.bucketId] = (extras[a.bucketId] ?? 0) + a.minutesGranted;
  }
  return extras;
}

/**
 * The caps to join a guardian's progress view against — `undefined` (raw,
 * uncapped meters) whenever Counted times is currently PAUSED
 * (`buckets.enabled === false`). Mirrors `GiveTime`'s own
 * `bucketsPolicy?.enabled ? bucketsPolicy.buckets : []` guard (found in
 * review: `Home.tsx`/`Approvals.tsx` passed `buckets?.buckets` straight
 * through with no such check, so a paused set still drew a "45m of 1h"-style
 * wall the device was no longer enforcing at all). `groupProgressRows`
 * already renders an unmatched/uncapped group honestly (raw meters, no "of
 * X"), so passing `undefined` here — rather than hiding the row outright —
 * keeps what was actually spent visible while the cap itself goes quiet.
 */
export function groupProgressBuckets(buckets: BucketsPolicy | undefined): AppBucketRule[] | undefined {
  return buckets?.enabled ? buckets.buckets : undefined;
}

/**
 * Join STATUS `groups` against the saved `buckets` policy — and, optionally,
 * today's guardian-granted extras (`groupExtrasToday`) so the row already
 * carries what `groupProgressLine` needs to keep the cap honest. Absent/empty
 * STATUS groups (a ward whose device predates named times, or simply hasn't
 * reported one yet) yields `[]` — the caller shows nothing new, never a row
 * of zeros implying a group that isn't actually being metered.
 */
export function groupProgressRows(
  statusGroups: StatusGroup[] | undefined,
  buckets: AppBucketRule[] | undefined,
  extraMinutesToday?: Record<string, number>,
): GroupProgressRow[] {
  if (!statusGroups || statusGroups.length === 0) return [];
  const byId = new Map((buckets ?? []).map((b) => [b.id, b]));
  return statusGroups.map((g) => {
    const rule = byId.get(g.id);
    const extra = extraMinutesToday?.[g.id];
    return {
      id: g.id,
      label: rule?.label ?? g.id,
      daySecs: g.daySecs,
      weekSecs: g.weekSecs,
      dailyMinutes: rule?.dailyMinutes,
      weeklyMinutes: rule?.weeklyMinutes,
      extraMinutesToday: extra ? extra : undefined,
    };
  });
}

/** "45m", "1h", "2h 10m" — never a raw minute count past the hour, and never
 *  a decimal. */
function fmtMinutes(mins: number): string {
  const m = Math.max(0, Math.round(mins));
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  return rem === 0 ? `${h}h` : `${h}h ${rem}m`;
}

/**
 * "Play — 45m of 1h today · 2h of 5h this week". An axis renders ONLY when
 * the family actually capped it — a bare "45m today" with no "of X" would
 * read as a cap that doesn't exist. A group STATUS reports but the policy
 * never capped at all (unknown to this save, or an old bucket the device
 * still remembers) shows both raw meters uncapped instead — honest about
 * what's spent even with nothing to measure it against.
 *
 * When the guardian granted extra today (`row.extraMinutesToday`), it's added
 * onto BOTH capped axes before the "of X" is built, and a trailing note names
 * it — "Play — 30m of 30m today · 30m of 30m this week (includes 15m extra
 * you gave)". Without this a spend the guardian personally approved reads as
 * a breach of a cap that no longer applies (M-2, hardware round 2026-08-03):
 * STATUS's raw meters are correct (they're what the device actually spent),
 * only the BASE cap joined against them was stale.
 */
export function groupProgressLine(row: GroupProgressRow): string {
  const dayMin = row.daySecs / 60;
  const weekMin = row.weekSecs / 60;
  const extra = row.extraMinutesToday ?? 0;
  const uncapped = row.dailyMinutes == null && row.weeklyMinutes == null;
  const parts: string[] = [];
  if (row.dailyMinutes != null) {
    parts.push(`${fmtMinutes(dayMin)} of ${fmtMinutes(row.dailyMinutes + extra)} today`);
  } else if (uncapped) {
    parts.push(`${fmtMinutes(dayMin)} today`);
  }
  if (row.weeklyMinutes != null) {
    parts.push(`${fmtMinutes(weekMin)} of ${fmtMinutes(row.weeklyMinutes + extra)} this week`);
  } else if (uncapped) {
    parts.push(`${fmtMinutes(weekMin)} this week`);
  }
  const line = `${row.label} — ${parts.join(" · ")}`;
  return extra > 0 ? `${line} (includes ${fmtMinutes(extra)} extra you gave)` : line;
}
