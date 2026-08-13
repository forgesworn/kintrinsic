// The guardian-facing "out-of-hours" week line — spec 2026-08-03,
// `spec/contract.md`'s `alwaysavailable` clause. Read `unrecognisedTime.ts`
// first: this is its week-scoped mirror image, and reuses its tz/day-key
// machinery rather than inventing a second convention.
//
// STATUS carries `outOfHoursWeekSecs` / `outOfHoursNightsWeek` as cumulative
// WEEK-KEYED COUNTERS with no date attached to the pair itself — only the
// STATUS's own `dayKey` says when the report was made. Without a freshness
// guard, a device that reported once late in its week and then went dark
// (off, uninstalled, out of battery) would go on contributing that week's
// figures forever, even after the ward's own device rolled its ledger over
// at the week boundary and would show nothing at all for the new week — the
// guardian sees a number the ward cannot see because it is no longer true
// (review finding C1, 2026-08-04).

import type { Child, Policy } from "./types";
import type { DeviceStatus } from "../wire/status";
import { currentDayKey } from "../wire/grant";

const WEEKDAY_ORDER = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** 0=Sun..6=Sat for `nowSecs` in `tz` — falls back to the runtime's local tz
 *  on an unknown/unparseable zone, same discipline as `currentDayKey`. */
function weekdayIndex(nowSecs: number, tz?: string): number {
  try {
    const label = new Intl.DateTimeFormat("en-US", { timeZone: tz, weekday: "short" }).format(
      new Date(nowSecs * 1000),
    );
    const idx = WEEKDAY_ORDER.indexOf(label);
    if (idx >= 0) return idx;
  } catch {
    // fall through to the local-tz fallback below
  }
  const label = new Intl.DateTimeFormat("en-US", { weekday: "short" }).format(
    new Date(nowSecs * 1000),
  );
  return Math.max(0, WEEKDAY_ORDER.indexOf(label));
}

/**
 * The seven `dayKey`s (in `tz`) belonging to the calendar week containing
 * `nowSecs`, starting on `weekStart`. Computed the same way charterd derives
 * a device's own `weekKey` (back to the most recent `weekStart` day, forward
 * six more) — but as a day-key SET rather than one key, since the guard below
 * needs "is this dayKey inside the current week", not "does it equal the
 * week's first day".
 */
function currentWeekDayKeys(nowSecs: number, tz: string, weekStart: "sun" | "mon"): Set<string> {
  const startIdx = weekStart === "sun" ? 0 : 1;
  const daysBack = (weekdayIndex(nowSecs, tz) - startIdx + 7) % 7;
  const keys = new Set<string>();
  for (let i = -daysBack; i < 7 - daysBack; i += 1) {
    keys.add(currentDayKey(nowSecs + i * 86_400, tz));
  }
  return keys;
}

/** The device-scope policy's week-start day — mirrors `Budget.weekStart`'s
 *  own default ("mon"), the same convention `BucketsPolicy.weekStart` and the
 *  weekly picture's chart already follow. */
export function outOfHoursGuardWeekStart(policy: Policy | undefined): "sun" | "mon" {
  return policy?.budget?.weekStart ?? "mon";
}

/** One paired, fresh device's contribution to the week's out-of-hours line. */
export interface OutOfHoursContribution {
  deviceId: string;
  secs: number;
  nights: number;
}

/**
 * One row per PAIRED device whose last STATUS's `dayKey` falls inside the
 * ward's CURRENT week (`tz`/`weekStart`, resolved by the caller the same way
 * `unrecognisedRows` resolves its own tz — see `unrecognisedGuardTz`). Both
 * conditions matter:
 *
 *  - **pairing**: a released device's last STATUS must not go on being
 *    counted forever just because Kintrinsic still remembers it.
 *  - **freshness**: a STATUS from a week that has since rolled over, on the
 *    ward's own device, must not still read as "this week" here.
 *
 * A device with no STATUS at all, or one whose `dayKey` is stale, contributes
 * nothing — silently, the same "absence is never a finding" discipline
 * `unrecognisedRows` uses.
 */
export function outOfHoursContributions(
  child: Child,
  deviceStatus: Record<string, DeviceStatus>,
  nowSecs: number,
  tz: string,
  weekStart: "sun" | "mon",
): OutOfHoursContribution[] {
  const week = currentWeekDayKeys(nowSecs, tz, weekStart);
  const out: OutOfHoursContribution[] = [];
  for (const d of child.devices) {
    if (d.pairing !== "paired" || !d.devicePubkey) continue;
    const status = deviceStatus[d.devicePubkey];
    if (!status || !week.has(status.dayKey)) continue;
    out.push({
      deviceId: d.id,
      secs: status.outOfHoursWeekSecs ?? 0,
      nights: status.outOfHoursNightsWeek ?? 0,
    });
  }
  return out;
}

/**
 * Fold fresh contributions into the week's totals: seconds SUM across
 * devices (a genuinely separate span of time on each), nights take the
 * largest single device's figure (STATUS carries no per-day detail this
 * could union across devices without risking a double-counted night).
 *
 * This is exactly why the freshness guard in `outOfHoursContributions`
 * matters MORE for nights than for the seconds sum: a stale device folded
 * into a SUM merely adds a bounded, eventually-corrected error, but folded
 * into a MAX it can PIN the figure indefinitely — a device that reported "3
 * nights" once and then went dark keeps `Math.max(0, 3)` reading "3" forever,
 * even once every live device genuinely reads 0. Dropping the stale device
 * BEFORE the fold, rather than trying to discount it after, is the only way
 * `Math.max` stays honest.
 */
export function outOfHoursTotals(contributions: OutOfHoursContribution[]): {
  nights: number;
  secs: number;
} {
  let nights = 0;
  let secs = 0;
  for (const c of contributions) {
    secs += c.secs;
    nights = Math.max(nights, c.nights);
  }
  return { nights, secs };
}
