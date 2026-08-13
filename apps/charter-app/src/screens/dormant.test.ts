import { describe, expect, it } from "vitest";
import { buildLifeline, buildSchedule, fromDormant, isDormant, toDormant } from "./Limits";
import { scheduleToGrant } from "../wire/clause";
import { WEEKDAYS, type Schedule } from "../domain/types";

const AT = 1_782_752_400;
const base = (over: Partial<Schedule> = {}): Schedule => ({
  tz: "Europe/London",
  weekly: { mon: [{ start: "16:00", end: "18:00" }] },
  ...over,
});

describe("the dormant posture — 'off unless I open it'", () => {
  /**
   * THE load-bearing invariant. The guardian's screen calls a device dormant by
   * reading the draft; the device is actually shut by the wire clause saying
   * `paused: true`. If those two ever disagree, Kintrinsic says "Off" over a
   * tablet that is quietly allowing time — the exact failure the posture exists
   * to prevent, and one nobody would notice until a child did.
   */
  it("agrees with what actually goes on the wire, in every shape", () => {
    const shapes: Schedule[] = [
      base(),
      base({ weekly: {} }),
      base({ weekly: {}, paused: true }),
      base({ paused: true }),
      base({ weekly: {}, overrides: { "2026-07-01": [{ start: "09:00", end: "10:00" }] } }),
      base({ weekly: {}, overrides: { "2026-07-01": [] } }),
      base({ weekly: { mon: [] , tue: [] } }),
    ];
    for (const s of shapes) {
      expect(isDormant(s), JSON.stringify(s)).toBe(scheduleToGrant(s, AT).paused === true);
    }
  });

  it("is not the same as app-paused, which means the opposite", () => {
    // App-paused lifts the schedule (always allowed). A guardian who has lifted
    // the schedule has not put the device to sleep — they've done the reverse.
    expect(isDormant(base({ weekly: {}, paused: true }))).toBe(false);
  });

  it("an override window keeps it awake", () => {
    // One-off allowed time on a date means there IS allowed time, and
    // scheduleToGrant agrees by not emitting block-all.
    const s = base({ weekly: {}, overrides: { "2026-07-01": [{ start: "09:00", end: "10:00" }] } });
    expect(isDormant(s)).toBe(false);
  });

  it("going dormant clears one-off days too", () => {
    // A forgotten override would wake the device on a date nobody remembers
    // setting — a spare tablet that turns itself on is the whole problem.
    const s = toDormant(base({ overrides: { "2026-07-01": [{ start: "09:00", end: "10:00" }] } }));
    expect(isDormant(s)).toBe(true);
    expect(scheduleToGrant(s, AT).paused).toBe(true);
    expect(s.tz).toBe("Europe/London");
  });

  it("waking it gives a real week back, not a blank editor", () => {
    // Returning to an empty week would still MEAN dormant, leaving the toggle
    // looking broken.
    const woken = fromDormant(toDormant(base()));
    expect(isDormant(woken)).toBe(false);
    for (const d of WEEKDAYS) expect(woken.weekly[d]?.length).toBe(1);
    expect(scheduleToGrant(woken, AT).paused).toBeUndefined();
  });

  it("survives a round trip", () => {
    expect(isDormant(toDormant(fromDormant(base())))).toBe(true);
  });

  /**
   * "Save changes" greys out when the draft matches what the app rebuilds from
   * the saved policy. `toDormant` used to return `weekly: {}` while
   * `buildSchedule` reconstructs seven empty days, so the two never compared
   * equal: the card stayed dirty forever and the save looked like it hadn't
   * taken (decented, 2026-07-29).
   */
  it("produces a week already in saved shape, so the card can settle", () => {
    const dormant = toDormant(base());
    expect(JSON.stringify(buildSchedule(dormant))).toBe(JSON.stringify(dormant));
  });

  /**
   * A ward whose schedule was NEVER signed normalises to the same seven empty
   * days as a deliberately-dormant one. Telling them apart is not cosmetic:
   * reading the unauthored case as "Off" put "Off — you open it when needed" on
   * the guardian's screen over a tablet nothing was governing.
   */
  it("cannot tell 'never set up' from 'deliberately off' by shape alone", () => {
    // Both are dormant BY SHAPE — which is exactly why the card must consult
    // whether a clause was ever signed (`policy.schedule !== undefined`)
    // rather than trusting this predicate on its own.
    expect(isDormant(buildSchedule(undefined))).toBe(true);
    expect(isDormant(toDormant(base()))).toBe(true);
    // tz is normalised away: an unauthored draft takes the MACHINE's zone, so
    // comparing it raw passes in Europe/London and fails on a UTC CI runner.
    // The claim under test is about the week's shape, not the timezone.
    const shape = (s: Schedule) => JSON.stringify({ ...s, tz: "«tz»" });
    expect(shape(buildSchedule(undefined))).toBe(shape(toDormant(base())));
  });
});

describe("the emergency-unlock safety net", () => {
  /**
   * A dormant device is the case that makes this load-bearing: it is locked by
   * default, so the ONLY ways in are the relay (needs a known network) and
   * break-glass (needs nothing). Move it to unfamiliar Wi-Fi with the net down
   * and it cannot be opened by anyone — the guardian's grant is published and
   * never collected, and the lock screen offers no way to join a network.
   *
   * MUST match `BreakGlassCfg::safety_net()` in charter-proto. Two languages,
   * one decision: if these drift, the guardian's screen and the device disagree
   * about whether a ward has a way out.
   */
  it("is on by default, opening the whole device for 10 minutes", () => {
    const bg = buildLifeline(undefined).breakGlass;
    expect(bg).toEqual({ enabled: true, scope: "full", durationMinutes: 10 });
  });

  it("respects a guardian who deliberately turned it off", () => {
    // A default is not a law. If this ever coerced back to true the toggle
    // would be decorative, which is worse than not offering it.
    const off = buildLifeline({
      numbers: [],
      breakGlass: { enabled: false, scope: "full", durationMinutes: 10 },
    }).breakGlass;
    expect(off?.enabled).toBe(false);
  });
});
