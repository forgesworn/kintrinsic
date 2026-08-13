// The guardian-facing "unrecognised time" line — spec §2.3/§2.5,
// `spec/contract.md`'s `unrecognisedTodaySecs`. Read the contract entry
// before touching this file: the counter's SCOPE was revised three times
// across review, and it is now, deliberately, a FLOOR — never a total.
//
// Copy here must stay warm and non-accusatory: it names a fact ("Kintrinsic
// couldn't identify this much time") that starts a conversation, never a
// verdict. It must also never overstate completeness — the explainer is
// binding, not decoration (contract's "must not present this number as a
// complete account" rule).

import type { Child, Policy } from "./types";
import type { DeviceStatus } from "../wire/status";
import { currentDayKey } from "../wire/grant";

/**
 * The tz the DAY GUARD (below) must compare against — deliberately NOT
 * `domain/childTz.ts`'s `resolveChildTz` (review finding, New-1).
 * charterd stamps a STATUS's `dayKey` using its own `enforcement_tz_of`:
 * `schedule.tz ?? budget.tz ?? UTC` — the buckets clause's tz is never
 * part of it. `resolveChildTz` deliberately DOES fall back to
 * `buckets.tz` (a correct, separate rule for a grant's expiry — see that
 * file's own doc), which agrees with the device for any ward that has a
 * schedule or budget, but DISAGREES for a named-times-only ward (buckets,
 * no schedule/budget) — exactly this feature's own minimal configuration,
 * and exactly Robin's laptop for the hardware round. There the device
 * stamps `dayKey` in UTC while `resolveChildTz` would return the buckets
 * tz, and the mismatch window is the ward's own UTC offset: measured, a
 * fresh, correct 2h report at 23:30 UTC reads as gone under Europe/London;
 * a New York ward loses the row every evening from 20:00 to midnight
 * local. Mirror the device exactly here instead.
 */
export function unrecognisedGuardTz(policy: Policy | undefined): string {
  return policy?.schedule?.tz ?? policy?.budget?.tz ?? "UTC";
}

/** One device's unrecognised-time report, ready to render. Only ever built
 *  for a device that actually reported something > 0 — see `unrecognisedRows`. */
export interface UnrecognisedRow {
  deviceId: string;
  /** The device's own label, e.g. "Sam's laptop" — used only when the ward
   *  has more than one device (see `unrecognisedLine`). */
  deviceLabel: string;
  secs: number;
}

/**
 * One row per paired device that reported unrecognised time TODAY, greater
 * than zero. A device that hasn't reported the field at all (never sent it,
 * predates charterd 0.7.5, or is an Android phone — the field is Linux-only
 * by construction) yields no row, and neither does a device that genuinely
 * reported zero: absence is never a finding, and zero is never a headline.
 *
 * DAY GUARD (review findings F5, then New-1): a STATUS's `dayKey` must equal
 * the ward's CURRENT day (`currentDayKey`, in `tz` — never the guardian
 * phone's) at `nowSecs`, or the row is dropped. Without this, a guardian who
 * has the PWA open across the ward's midnight would keep reading last
 * night's figure inside a card headed "Today" until the device next reports
 * — the exact "confident wrong number" failure class this feature exists to
 * avoid on the other side of the same card. `tz` MUST be `unrecognisedGuardTz`'s
 * output, never `resolveChildTz` — see that function's own doc for why the
 * two disagree for exactly this feature's minimal (buckets-only) config.
 */
export function unrecognisedRows(
  child: Child,
  deviceStatus: Record<string, DeviceStatus>,
  nowSecs: number,
  tz: string,
): UnrecognisedRow[] {
  const today = currentDayKey(nowSecs, tz);
  const rows: UnrecognisedRow[] = [];
  for (const d of child.devices) {
    if (d.pairing !== "paired" || !d.devicePubkey) continue;
    const status = deviceStatus[d.devicePubkey];
    const secs = status?.unrecognisedTodaySecs;
    if (secs != null && secs > 0 && status?.dayKey === today) {
      rows.push({ deviceId: d.id, deviceLabel: d.label, secs });
    }
  }
  return rows;
}

/** "45m", "1h", "3h 12m" — mirrors `groupProgress.ts`'s own duration style,
 *  rounded to the minute (never a raw seconds count, never a decimal). A
 *  device that reported a positive-but-sub-minute value still reads as "1m"
 *  rather than "0m", which would contradict the ">0" gate that got it here. */
function fmtDuration(secs: number): string {
  const mins = Math.max(1, Math.round(secs / 60));
  if (mins < 60) return `${mins}m`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return m === 0 ? `${h}h` : `${h}h ${m}m`;
}

/**
 * "3h 12m Kintrinsic didn't recognise" — named to the specific device ("...on
 * Sam's laptop") only when the ward has more than one, so a single-device
 * family never reads a redundant device name for the one device they have.
 */
export function unrecognisedLine(row: UnrecognisedRow, multiDevice: boolean): string {
  const time = fmtDuration(row.secs);
  return multiDevice
    ? `${time} Kintrinsic didn't recognise on ${row.deviceLabel}`
    : `${time} Kintrinsic didn't recognise`;
}

/**
 * The one-line explainer shown once alongside the unrecognised line(s) — not
 * per row, so it reads as an explanation of the NUMBER rather than an
 * accusation repeated per device. States plainly (1) what the time IS —
 * software installed under the ward's own account that no rule currently
 * names — (2) a BENIGN reading of that fact (review finding, F7: the
 * mechanism alone offers no innocent explanation, and on Approvals this sits
 * directly above a pending ask like "Sam asks for 10 more minutes" — the
 * worst place for it to read as evidence rather than context; contract's own
 * gloss on this field is "grounds for a conversation, not a dossier", and the
 * copy should say so) — and (3) that it is not a complete account: contract's
 * binding "floor, not a total" rule, kept verbatim, unchanged by the F7 fix.
 * "often nothing surprising", not "often just something they installed
 * themselves" (follow-up polish): F3's own principle is that the mark
 * doesn't know WHO installed anything, so the benign reading shouldn't
 * re-assert agency either — the softer phrasing keeps the same reassurance
 * without implying the ward is who put it there.
 */
export const UNRECOGNISED_EXPLAINER =
  "Software installed under their own account that none of your rules name yet — often nothing surprising. Worth asking about. Kintrinsic can't see everything, so this is a floor, not the whole picture.";
