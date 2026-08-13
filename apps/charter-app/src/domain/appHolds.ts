// "Let him on Vanadium — but just for an hour."
//
// The standing per-app toggle is the only lever the Apps list had, so a guardian
// who wanted a one-off flipped a RULE: decented let Robin onto Vanadium on
// 2026-08-02 and immediately had to start remembering to put it back. That is
// the same shape the `gift` clause fixed on the time dimension and the install
// window fixed on the install dimension — a temporary intent with only a
// permanent control to express it.
//
// A hold is the temporary form. It is carried on the wire as an ABSOLUTE
// instant and enforced by the ward's own device, never by a timer here: this app
// is a web app on a guardian's phone, and the OS freezes its timers the moment
// it is backgrounded (see mycharter-staleness-traps). A hold that depended on
// this screen being awake would be a loosening that silently never ended.

import type { AppHold, AppsPolicy, HoldState, Schedule, Weekday } from "./types";
import type { PolicyOverride } from "./effectivePolicy";

/** The longest hold the ward will honour — mirrored from `MAX_APP_HOLD_SECS`. */
export const MAX_HOLD_SECONDS = 24 * 60 * 60;

/** The most holds one clause may carry — mirrored from `MAX_APP_HOLDS`. */
export const MAX_HOLDS = 64;

/** What the sheet offers. "Predefined time", as decented put it — one tap. */
export const HOLD_PRESETS: { minutes: number; label: string }[] = [
  { minutes: 15, label: "15 min" },
  { minutes: 30, label: "30 min" },
  { minutes: 60, label: "1 hour" },
  { minutes: 120, label: "2 hours" },
];

/**
 * Drop everything the ward would drop anyway: holds already over, holds reaching
 * past the cap, and anything past the 64th. Applied on render AND before
 * publishing, so what this screen shows and what the phone enforces can never
 * disagree.
 */
export function pruneHolds(
  holds: AppHold[] | undefined,
  nowUnix: number,
): AppHold[] {
  if (!holds?.length) return [];
  return holds
    .filter((h) => h.pkg.trim().length > 0)
    .filter((h) => h.untilUnix > nowUnix)
    .filter((h) => h.untilUnix <= nowUnix + MAX_HOLD_SECONDS)
    .slice(0, MAX_HOLDS);
}

/** The live hold on one app, if any. */
export function holdFor(
  apps: AppsPolicy,
  pkg: string,
  nowUnix: number,
): AppHold | undefined {
  // Last writer wins, matching the device: the guardian's most recent word
  // about an app is the one being enforced, so it is the one to show.
  const live = pruneHolds(apps.holds, nowUnix).filter((h) => h.pkg === pkg);
  return live.length ? live[live.length - 1] : undefined;
}

/**
 * Put a hold on an app, replacing any it already had. `untilUnix` of null lifts
 * it. Expired holds are pruned on the way through so the list cannot grow.
 */
export function putHold(
  apps: AppsPolicy,
  pkg: string,
  state: HoldState,
  untilUnix: number | null,
  nowUnix: number,
): AppsPolicy {
  const rest = pruneHolds(apps.holds, nowUnix).filter((h) => h.pkg !== pkg);
  const holds = untilUnix === null ? rest : [...rest, { pkg, state, untilUnix }];
  return { ...apps, holds };
}

/** What one `composeDeviceHold` compose produces — ready to hand straight to
 *  `savePolicy(childId, scopeId, { apps, deviceOverrides })`. */
export interface ComposedHold {
  apps: AppsPolicy;
  deviceOverrides: Record<string, PolicyOverride> | undefined;
}

/**
 * Overlay a hold against the CORRECT baseline for a SPECIFIC device — the
 * one asking, when the caller knows which device that is (an app.open ask
 * always does) — mirroring exactly what a manual hold set from Limits would
 * do for that same device (the C2 fix, review round 1, 2026-08-03).
 *
 * A device that SPLITS `apps` never reads base at all
 * (`effectivePolicyForDevice` picks EITHER base OR its own override, never
 * both), so a hold meant for that device must land in ITS OWN override or it
 * silently never reaches the phone that asked. A device that does NOT split
 * `apps` reads base, so the hold lands there instead. Either way, every
 * OTHER device's override — for `apps` or any other control — travels
 * through BYTE-IDENTICAL: this is the one thing that must never regress,
 * because a save that omits `deviceOverrides` entirely (the original bug)
 * makes `pickOverrides` flatten every split silently the moment ANY save
 * touches the same control (see `store.tsx`'s `savePolicy` doc on this exact
 * failure mode — a laptop's own Steam block lifted by a phone's "open
 * Minecraft" approval).
 */
export function composeDeviceHold(
  base: { apps: AppsPolicy; deviceOverrides?: Record<string, PolicyOverride> },
  deviceId: string | undefined,
  pkg: string,
  state: HoldState,
  untilUnix: number,
  nowUnix: number,
): ComposedHold {
  const splitApps = deviceId ? base.deviceOverrides?.[deviceId]?.apps : undefined;
  if (deviceId && splitApps) {
    return {
      // Unchanged — but it must still ride along in the save (only a
      // dimension actually PRESENT gets its overrides carried at all; see
      // `pickOverrides`), and every unsplit device reads exactly this.
      apps: base.apps,
      deviceOverrides: {
        ...base.deviceOverrides,
        [deviceId]: {
          ...base.deviceOverrides![deviceId],
          apps: putHold(splitApps, pkg, state, untilUnix, nowUnix),
        },
      },
    };
  }
  return {
    apps: putHold(base.apps, pkg, state, untilUnix, nowUnix),
    // Untouched: every device that DOES split some control keeps it exactly
    // as saved.
    deviceOverrides: base.deviceOverrides,
  };
}

/**
 * The app's standing state — what it is with every hold set aside. This is what
 * the row's toggle shows and edits.
 */
export function standingState(apps: AppsPolicy, pkg: string): HoldState {
  if (apps.posture === "allowlist") {
    return apps.allowed.includes(pkg) ? "allowed" : "blocked";
  }
  return apps.blocked.includes(pkg) ? "blocked" : "allowed";
}

/** What the app actually IS right now — the standing state, or its live hold. */
export function effectiveState(
  apps: AppsPolicy,
  pkg: string,
  nowUnix: number,
): HoldState {
  return holdFor(apps, pkg, nowUnix)?.state ?? standingState(apps, pkg);
}

/**
 * What a hold on this app would do, given what it is now. A blocked app is one
 * you'd let through; an open one is one you'd pause.
 */
export function holdDirection(
  apps: AppsPolicy,
  pkg: string,
  nowUnix: number,
): HoldState {
  return effectiveState(apps, pkg, nowUnix) === "blocked" ? "allowed" : "blocked";
}

/**
 * "59 minutes left". Rounds UP while a minute is still running, so the number
 * never claims less time than the phone will actually give — a guardian reading
 * "0 minutes" beside a working app would think it had broken. Below a minute it
 * stops counting rather than flickering through the seconds.
 */
export function holdLabel(secondsLeft: number): string {
  if (secondsLeft <= 0) return "over";
  if (secondsLeft < 60) return "under a minute left";
  const m = Math.ceil(secondsLeft / 60);
  if (m < 60) return `${m} min left`;
  const h = Math.floor(m / 60);
  const rem = m % 60;
  if (rem === 0) return `${h} hour${h === 1 ? "" : "s"} left`;
  return `${h}h ${rem}m left`;
}

/** How the row reads while a hold is running: "Allowed · 59 min left". */
export function holdRowLabel(hold: AppHold, nowUnix: number): string {
  const word = hold.state === "allowed" ? "Allowed" : "Paused";
  return `${word} · ${holdLabel(hold.untilUnix - nowUnix)}`;
}

/** Indexed by `Date.getDay()`, which starts on Sunday — WEEKDAYS starts Monday. */
const BY_GETDAY: Weekday[] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/**
 * The end of today's last allowed window, as an absolute instant — the
 * "Until bedtime" preset.
 *
 * Returns null when there is nothing to stand behind: no schedule, no window
 * today, or the last one has already passed. The preset is HIDDEN in that case
 * rather than quietly meaning midnight. A button that says "bedtime" and means
 * something else is worse than no button.
 *
 * Resolved against the GUARDIAN's clock, like the install window. For a family
 * in one house that is right; across time zones it is off by the offset, which
 * is a real limit and a worse one to solve with a time-zone picker nobody asked
 * for.
 */
export function untilBedtime(
  schedule: Schedule | undefined,
  now: Date,
  capSeconds = MAX_HOLD_SECONDS,
): number | null {
  if (!schedule || schedule.paused) return null;
  const iso = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  // A one-off override for today REPLACES the weekly windows, exactly as the
  // schedule editor reads it — otherwise "bedtime" would quote a rule that is
  // not in force tonight.
  const windows =
    schedule.overrides?.[iso] ?? schedule.weekly[BY_GETDAY[now.getDay()]] ?? [];
  if (!windows.length) return null;
  const nowUnix = Math.floor(now.getTime() / 1000);
  let best: number | null = null;
  for (const w of windows) {
    const [h, m] = w.end.split(":").map((n) => Number.parseInt(n, 10));
    if (!Number.isFinite(h) || !Number.isFinite(m)) continue;
    const end = new Date(now);
    end.setHours(h, m, 0, 0);
    const unix = Math.floor(end.getTime() / 1000);
    if (unix > nowUnix && (best === null || unix > best)) best = unix;
  }
  if (best === null) return null;
  // Never offer a preset the device would refuse outright.
  return best - nowUnix <= capSeconds ? best : null;
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}
