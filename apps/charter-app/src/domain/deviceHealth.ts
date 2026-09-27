// Health notes for a device's card — the STATUS fields that were parsed
// (`wire/status.ts`) but never shown anywhere (review 2026-09-27, G-2 /
// opportunity 1): `pausedByAdmin`, `enforcementGapSecs`, `relayUnreachablePolls`,
// `transportUnavailable`, plus the guardian-local `clockWentBackwards` note
// (G-1, see `store/deviceStatusAdmit.ts`). Linux goes to real trouble to put
// the first four on the wire — "the phone is the only place a parent looks" —
// and a guardian who can't see them sees a paused device rendered as an
// ordinary unlocked one. `usageUnsaved` joins the same set: the ward's own
// fail-safe pause when it can no longer save its usage record.
//
// All inputs are optional and independent: an older ward that omits a field
// (or every field) renders no note for it at all, exactly as before this
// module existed. Nothing here invents a wire field a ward doesn't send.
//
// SECOND ROUND (review 2026-09-27, F5): every note above is phrased in the
// present tense ("nothing is being enforced right now"), which was a false
// claim once the STATUS it's drawn from is hours old — worse once G-1's own
// flag or a stale multi-day reading is involved. Every note now carries the
// age of the reading it came from, and the card as a whole is told when its
// evidence is too old to trust at all.

import { STATUS_FRESHNESS_SECS } from "../store/liveStatus";

export type DeviceHealthInput = {
  pausedByAdmin?: boolean;
  /** Seconds, ordinary state is absent or 0 — never rendered as "0 h". */
  enforcementGapSecs?: number;
  /** Ordinary state is absent or 0. */
  relayUnreachablePolls?: number;
  transportUnavailable?: boolean;
  /** True only when the ward's usage record has failed to save continuously
   *  for five minutes or more — the ward holds the child's screen time
   *  paused as a fail-safe while this is true. */
  usageUnsaved?: boolean;
  /** Guardian-local: set by `mergeDeviceStatus` when this reading was
   *  accepted despite a `ts` lower than the last one stored. Not a wire field. */
  clockWentBackwards?: boolean;
  /** Unix SECONDS (device clock) this reading was taken — `DeviceStatus.ts`.
   *  Absent only for a fixture built with no real feed at all, in which case
   *  no age is shown and the card is never marked stale on its account. */
  ts?: number;
};

export type DeviceHealthNoteKey =
  | "paused"
  | "enforcementGap"
  | "relayTrouble"
  | "transportUnavailable"
  | "usageUnsaved"
  | "clockBackwards";

export type DeviceHealthNote = {
  key: DeviceHealthNoteKey;
  text: string;
  tone: "warn" | "neutral";
};

export type DeviceHealthNotes = {
  notes: DeviceHealthNote[];
  /** True when `status.ts` is older than `HEALTH_STALE_SECS` — the notes
   *  above may no longer describe the device's current state, and the card
   *  should say so rather than assert it as "right now". */
  stale: boolean;
};

/** A reading older than this is stale for health-note purposes. Reuses the
 *  same boundary the rest of the app already treats a STATUS feed as "not
 *  live" (`STATUS_FRESHNESS_SECS`, `store/liveStatus.ts`: three missed
 *  heartbeats at the ward's own cadence, `STATUS_HEARTBEAT_SECS = 60` in
 *  `linux/crates/charterd/src/runtime.rs`) — one boundary for "can this be
 *  trusted as current" everywhere in the app, rather than a second
 *  independently-tuned number here. */
export const HEALTH_STALE_SECS = STATUS_FRESHNESS_SECS;

/** "2 h 10 min" / "45 min" / "1 h" — a duration a parent would say out loud,
 *  never "0 h" (a duration this small still reads as "45 min", not nothing). */
export function humaniseDuration(totalSecs: number): string {
  const s = Math.max(0, Math.round(totalSecs));
  const h = Math.floor(s / 3600);
  const m = Math.round((s % 3600) / 60);
  if (h === 0) return `${Math.max(m, 1)} min`;
  return m === 0 ? `${h} h` : `${h} h ${m} min`;
}

/**
 * "moments ago" / "12 min ago" / "3 h ago" / "as of 14:02" — the age of a
 * reading taken at `tsSecs` (device clock, unix seconds), as of `nowMs`
 * (guardian's own clock). Falls back to a clock-time reading rather than a
 * growing "N h ago" once the age passes a day: "19 h ago" reads as freshly
 * counted, "31 h ago" does not, and a wall-clock stamp is the honest way to
 * say "this is from a while back" without pretending to a precision (exact
 * hours) a day-old reading no longer has.
 */
export function statusAgeLabel(tsSecs: number, nowMs: number): string {
  const secs = Math.max(0, Math.round(nowMs / 1000 - tsSecs));
  if (secs < 90) return "moments ago";
  if (secs < 3600) return `${Math.round(secs / 60)} min ago`;
  if (secs < 24 * 3600) return `${Math.round(secs / 3600)} h ago`;
  const d = new Date(tsSecs * 1000);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `as of ${hh}:${mm}`;
}

/**
 * The compact set of health notes to render on a device card, in a fixed,
 * stable order (worst-first is not attempted — these are independent facts,
 * not a severity ranking). Absent/false/zero inputs contribute nothing, so an
 * older ward that reports none of this shows an empty list, unchanged from
 * before these fields existed.
 *
 * Every note names the age of the reading it's drawn from, and `stale` tells
 * the card when that reading itself is too old to trust as "right now".
 */
export function deviceHealthNotes(
  status: DeviceHealthInput | undefined,
  now: number = Date.now(),
): DeviceHealthNotes {
  if (!status) return { notes: [], stale: false };
  const age = status.ts !== undefined ? ` (${statusAgeLabel(status.ts, now)})` : "";
  const stale = status.ts !== undefined && now / 1000 - status.ts > HEALTH_STALE_SECS;
  const notes: DeviceHealthNote[] = [];
  if (status.pausedByAdmin) {
    notes.push({
      key: "paused",
      tone: "warn",
      text: `Paused by an administrator — nothing is being enforced right now${age}.`,
    });
  }
  if (status.enforcementGapSecs) {
    notes.push({
      key: "enforcementGap",
      tone: "warn",
      text: `Not enforced for ${humaniseDuration(status.enforcementGapSecs)} before it last started${age}.`,
    });
  }
  if (status.relayUnreachablePolls) {
    notes.push({
      key: "relayTrouble",
      tone: "warn",
      text: `Can't reach its relay (${status.relayUnreachablePolls} polls)${age}.`,
    });
  }
  if (status.transportUnavailable) {
    notes.push({
      key: "transportUnavailable",
      tone: "warn",
      text: `Can't connect: a machine-key problem. Cached rules still apply${age}.`,
    });
  }
  if (status.usageUnsaved) {
    notes.push({
      key: "usageUnsaved",
      tone: "warn",
      text: `Can't save screen-time records — screen time is paused until the device can save again (disk full?)${age}.`,
    });
  }
  if (status.clockWentBackwards) {
    notes.push({
      key: "clockBackwards",
      tone: "warn",
      text: `This device's clock went backwards — its reported time may be behind${age}.`,
    });
  }
  return { notes, stale };
}
