import { describe, expect, it } from "vitest";
import {
  composeDeviceHold,
  effectiveState,
  holdDirection,
  holdFor,
  holdLabel,
  holdRowLabel,
  MAX_HOLDS,
  MAX_HOLD_SECONDS,
  pruneHolds,
  putHold,
  standingState,
  untilBedtime,
} from "./appHolds";
import { effectivePolicyForDevice, pickOverrides, SPLITTABLE_CONTROLS } from "./effectivePolicy";
import type { AppsPolicy, Policy, Schedule } from "./types";

const NOW = 1_782_734_400;
const VANADIUM = "org.chromium.vanadium";

function blocklist(blocked: string[], holds?: AppsPolicy["holds"]): AppsPolicy {
  return { enabled: true, posture: "blocklist", blocked, allowed: [], holds };
}

function allowlist(allowed: string[], holds?: AppsPolicy["holds"]): AppsPolicy {
  return { enabled: true, posture: "allowlist", blocked: [], allowed, holds };
}

describe("pruneHolds", () => {
  it("drops holds that are already over", () => {
    const holds = [
      { pkg: "a.b", state: "allowed" as const, untilUnix: NOW - 1 },
      { pkg: "c.d", state: "allowed" as const, untilUnix: NOW + 60 },
    ];
    expect(pruneHolds(holds, NOW).map((h) => h.pkg)).toEqual(["c.d"]);
  });

  it("drops a hold reaching past what the device would honour", () => {
    const holds = [
      { pkg: "a.b", state: "allowed" as const, untilUnix: NOW + MAX_HOLD_SECONDS + 1 },
    ];
    expect(pruneHolds(holds, NOW)).toEqual([]);
    // …but exactly at the cap survives, matching the device's `<=`.
    expect(
      pruneHolds([{ pkg: "a.b", state: "allowed", untilUnix: NOW + MAX_HOLD_SECONDS }], NOW),
    ).toHaveLength(1);
  });

  it("bounds the list and ignores blank package names", () => {
    const many = Array.from({ length: MAX_HOLDS + 5 }, (_, i) => ({
      pkg: `app.${i}`,
      state: "blocked" as const,
      untilUnix: NOW + 60,
    }));
    expect(pruneHolds(many, NOW)).toHaveLength(MAX_HOLDS);
    expect(pruneHolds([{ pkg: "  ", state: "allowed", untilUnix: NOW + 60 }], NOW)).toEqual([]);
  });

  it("treats an absent list as no holds", () => {
    expect(pruneHolds(undefined, NOW)).toEqual([]);
  });
});

describe("what an app is", () => {
  it("reads the standing lists per posture", () => {
    expect(standingState(blocklist([VANADIUM]), VANADIUM)).toBe("blocked");
    expect(standingState(blocklist([]), VANADIUM)).toBe("allowed");
    expect(standingState(allowlist([VANADIUM]), VANADIUM)).toBe("allowed");
    expect(standingState(allowlist([]), VANADIUM)).toBe("blocked");
  });

  it("lets a live hold win over the standing rule, and only until it ends", () => {
    const apps = blocklist([VANADIUM], [
      { pkg: VANADIUM, state: "allowed", untilUnix: NOW + 3600 },
    ]);
    expect(effectiveState(apps, VANADIUM, NOW)).toBe("allowed");
    // AT the instant, not a tick later — the same boundary the device uses.
    expect(effectiveState(apps, VANADIUM, NOW + 3600)).toBe("blocked");
  });

  it("offers the opposite of what the app currently is", () => {
    expect(holdDirection(blocklist([VANADIUM]), VANADIUM, NOW)).toBe("allowed");
    expect(holdDirection(blocklist([]), VANADIUM, NOW)).toBe("blocked");
    // With a hold already open, the button would close it again.
    const held = blocklist([VANADIUM], [
      { pkg: VANADIUM, state: "allowed", untilUnix: NOW + 60 },
    ]);
    expect(holdDirection(held, VANADIUM, NOW)).toBe("blocked");
  });

  it("shows the last word about an app when there are two", () => {
    const apps = blocklist([VANADIUM], [
      { pkg: VANADIUM, state: "allowed", untilUnix: NOW + 60 },
      { pkg: VANADIUM, state: "blocked", untilUnix: NOW + 120 },
    ]);
    expect(holdFor(apps, VANADIUM, NOW)?.state).toBe("blocked");
  });
});

describe("putHold", () => {
  it("adds a hold and replaces the one an app already had", () => {
    const one = putHold(blocklist([VANADIUM]), VANADIUM, "allowed", NOW + 900, NOW);
    expect(one.holds).toEqual([{ pkg: VANADIUM, state: "allowed", untilUnix: NOW + 900 }]);
    const two = putHold(one, VANADIUM, "allowed", NOW + 3600, NOW);
    expect(two.holds).toHaveLength(1);
    expect(two.holds?.[0].untilUnix).toBe(NOW + 3600);
  });

  it("lifts a hold with null, leaving the standing rule alone", () => {
    const held = putHold(blocklist([VANADIUM]), VANADIUM, "allowed", NOW + 900, NOW);
    const lifted = putHold(held, VANADIUM, "allowed", null, NOW);
    expect(lifted.holds).toEqual([]);
    expect(lifted.blocked).toEqual([VANADIUM]);
  });

  it("never disturbs another app's hold", () => {
    const apps = putHold(
      putHold(blocklist([]), "a.b", "blocked", NOW + 60, NOW),
      "c.d",
      "allowed",
      NOW + 120,
      NOW,
    );
    expect(apps.holds?.map((h) => h.pkg).sort()).toEqual(["a.b", "c.d"]);
  });

  it("tidies dead holds as it goes, so the list cannot grow forever", () => {
    const stale = blocklist([], [{ pkg: "old.app", state: "allowed", untilUnix: NOW - 5 }]);
    expect(putHold(stale, "new.app", "blocked", NOW + 60, NOW).holds).toEqual([
      { pkg: "new.app", state: "blocked", untilUnix: NOW + 60 },
    ]);
  });
});

describe("holdLabel", () => {
  it("never claims less time than the phone will give", () => {
    // 58m01s must not read "58 min" — it rounds UP while a minute is running.
    expect(holdLabel(58 * 60 + 1)).toBe("59 min left");
    expect(holdLabel(59 * 60)).toBe("59 min left");
    // …and 60 rounded-up minutes reads as the hour it is.
    expect(holdLabel(59 * 60 + 1)).toBe("1 hour left");
  });

  it("stops counting rather than flickering through the last seconds", () => {
    expect(holdLabel(59)).toBe("under a minute left");
    expect(holdLabel(1)).toBe("under a minute left");
  });

  it("says nothing is left once it is over", () => {
    expect(holdLabel(0)).toBe("over");
    expect(holdLabel(-10)).toBe("over");
  });

  it("reads hours as hours", () => {
    expect(holdLabel(120 * 60)).toBe("2 hours left");
    expect(holdLabel(60 * 60)).toBe("1 hour left");
    expect(holdLabel(90 * 60)).toBe("1h 30m left");
  });

  it("says which way a hold goes in the row", () => {
    expect(holdRowLabel({ pkg: "a", state: "allowed", untilUnix: NOW + 1800 }, NOW))
      .toBe("Allowed · 30 min left");
    expect(holdRowLabel({ pkg: "a", state: "blocked", untilUnix: NOW + 1800 }, NOW))
      .toBe("Paused · 30 min left");
  });
});

describe("untilBedtime", () => {
  const at = (h: number, m = 0) => new Date(2026, 7, 2, h, m, 0, 0); // a Sunday
  const sched = (windows: { start: string; end: string }[]): Schedule => ({
    tz: "Europe/London",
    weekly: { sun: windows },
  });

  it("finds the end of today's last window", () => {
    const got = untilBedtime(sched([{ start: "09:00", end: "12:00" }, { start: "16:00", end: "20:15" }]), at(17));
    expect(got).not.toBeNull();
    expect(new Date(got! * 1000).getHours()).toBe(20);
    expect(new Date(got! * 1000).getMinutes()).toBe(15);
  });

  // The preset is HIDDEN rather than quietly meaning midnight — a button that
  // says "bedtime" and means something else is worse than no button.
  it("gives nothing to stand behind when there is nothing to stand behind", () => {
    expect(untilBedtime(undefined, at(17))).toBeNull();
    expect(untilBedtime(sched([]), at(17))).toBeNull();
    expect(untilBedtime({ tz: "Europe/London", weekly: {} }, at(17))).toBeNull();
    // Last window already gone.
    expect(untilBedtime(sched([{ start: "09:00", end: "12:00" }]), at(17))).toBeNull();
    // A lifted schedule has no bedtime to quote.
    expect(untilBedtime({ ...sched([{ start: "09:00", end: "20:00" }]), paused: true }, at(17)))
      .toBeNull();
  });

  it("prefers a one-off override for today over the weekly rule", () => {
    const withOverride: Schedule = {
      ...sched([{ start: "16:00", end: "20:00" }]),
      overrides: { "2026-08-02": [{ start: "16:00", end: "22:00" }] },
    };
    const got = untilBedtime(withOverride, at(17));
    expect(new Date(got! * 1000).getHours()).toBe(22);
  });

  it("refuses a bedtime the device would not honour", () => {
    // A one-minute cap makes even tonight's real bedtime out of reach.
    expect(untilBedtime(sched([{ start: "09:00", end: "20:00" }]), at(17), 60)).toBeNull();
  });
});

/**
 * C2 (review round 1, 2026-08-03): approving an app.open ask must compose
 * against the asking device's ACTUAL effective surface, and must never
 * silently flatten a sibling device's own split. Mirrors
 * `effectivePolicy.test.ts`'s N2 pattern — composed through `pickOverrides` +
 * `effectivePolicyForDevice` together, not `composeDeviceHold` in isolation,
 * because the original bug (omitting `deviceOverrides` from the `savePolicy`
 * call entirely) only shows up one layer up from where `composeDeviceHold`
 * itself lives.
 */
describe("composeDeviceHold (C2: the asking device's correct baseline)", () => {
  const basePolicy: Policy = {
    id: "p1",
    scope: { kind: "device" },
    apps: blocklist([VANADIUM], undefined),
  };

  it("a SPLIT asking device gets its own rule preserved plus the hold; an unsplit sibling is untouched", () => {
    const laptopOwnRule = blocklist(["com.valvesoftware.steam", VANADIUM]);
    const saved: Policy = {
      ...basePolicy,
      deviceOverrides: { laptop: { apps: laptopOwnRule } },
    };

    const composed = composeDeviceHold(
      { apps: saved.apps!, deviceOverrides: saved.deviceOverrides },
      "laptop",
      VANADIUM,
      "allowed",
      NOW + 1800,
      NOW,
    );

    // Mirror store.tsx's real save composition: `toSign` narrowed to the
    // dimensions this save actually signs, then resolved per device.
    const toSign: Policy = {
      ...saved,
      apps: composed.apps,
      deviceOverrides: pickOverrides(composed.deviceOverrides, [...SPLITTABLE_CONTROLS]),
    };

    const laptopEffective = effectivePolicyForDevice(toSign, "laptop");
    // Its own rule survives (Steam still blocked) AND carries the hold.
    expect(laptopEffective.apps!.blocked).toEqual(["com.valvesoftware.steam", VANADIUM]);
    expect(laptopEffective.apps!.holds).toEqual([
      { pkg: VANADIUM, state: "allowed", untilUnix: NOW + 1800 },
    ]);

    // An unsplit sibling reads base — untouched, no hold, no Steam block it
    // never had.
    const siblingEffective = effectivePolicyForDevice(toSign, "phone");
    expect(siblingEffective.apps).toEqual(basePolicy.apps);
    expect(siblingEffective.apps!.holds ?? []).toEqual([]);
  });

  it("an UNSPLIT asking device gets the hold on base; a DIFFERENT device's own split is never flattened", () => {
    const tabletOwnRule = blocklist(["com.game.other"]);
    const saved: Policy = {
      ...basePolicy,
      deviceOverrides: { tablet: { apps: tabletOwnRule } },
    };

    // The asking device ("phone") is NOT split — the hold must land on base.
    const composed = composeDeviceHold(
      { apps: saved.apps!, deviceOverrides: saved.deviceOverrides },
      "phone",
      VANADIUM,
      "allowed",
      NOW + 1800,
      NOW,
    );

    const toSign: Policy = {
      ...saved,
      apps: composed.apps,
      deviceOverrides: pickOverrides(composed.deviceOverrides, [...SPLITTABLE_CONTROLS]),
    };

    const phoneEffective = effectivePolicyForDevice(toSign, "phone");
    expect(phoneEffective.apps!.holds).toEqual([
      { pkg: VANADIUM, state: "allowed", untilUnix: NOW + 1800 },
    ]);

    // THE BUG this fixes: omitting deviceOverrides from the save flattened
    // every OTHER device's split the moment apps was signed at all. The
    // tablet's own rule must survive byte-identical, with NO hold (it never
    // asked).
    const tabletEffective = effectivePolicyForDevice(toSign, "tablet");
    expect(tabletEffective.apps).toEqual(tabletOwnRule);
    expect(tabletEffective.apps!.holds ?? []).toEqual([]);
  });

  it("with no asking deviceId at all, falls back to base (never crashes, never invents a split)", () => {
    const composed = composeDeviceHold(
      { apps: basePolicy.apps!, deviceOverrides: undefined },
      undefined,
      VANADIUM,
      "allowed",
      NOW + 900,
      NOW,
    );
    expect(composed.apps.holds).toEqual([{ pkg: VANADIUM, state: "allowed", untilUnix: NOW + 900 }]);
    expect(composed.deviceOverrides).toBeUndefined();
  });
});
