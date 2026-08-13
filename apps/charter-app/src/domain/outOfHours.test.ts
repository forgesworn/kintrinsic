import { describe, expect, it } from "vitest";
import {
  outOfHoursContributions,
  outOfHoursGuardWeekStart,
  outOfHoursTotals,
} from "./outOfHours";
import type { Child, Policy } from "./types";
import type { DeviceStatus } from "../wire/status";

const LAPTOP_KEY = "a".repeat(64);
const PHONE_KEY = "c".repeat(64);
const TZ = "UTC";

function child(over: Partial<Child> = {}): Child {
  return {
    id: "child_sam",
    name: "Sam",
    color: "#000",
    dependantPubkey: null,
    devices: [
      { id: "d_phone", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE_KEY },
    ],
    policies: [],
    ...over,
  };
}

function status(over: Partial<DeviceStatus> = {}): DeviceStatus {
  return {
    v: 1,
    subject: "b".repeat(64),
    machine: PHONE_KEY,
    ts: 1_700_000_000,
    dayKey: "2026-08-04", // Tuesday
    usedTodaySecs: 0,
    windowLeftSecs: 0,
    quotaLeftSecs: 0,
    effectiveSecs: 0,
    locked: false,
    source: "guardian",
    ...over,
  };
}

// Tuesday 2026-08-04, noon UTC — the week (Mon-start) runs 2026-08-03..2026-08-09.
const NOW_TUESDAY = Math.floor(Date.parse("2026-08-04T12:00:00Z") / 1000);
// A week later — the week has rolled over at least once since NOW_TUESDAY.
const NOW_NEXT_TUESDAY = Math.floor(Date.parse("2026-08-11T12:00:00Z") / 1000);

describe("outOfHoursGuardWeekStart", () => {
  it("defaults to Monday when the device policy sets nothing", () => {
    expect(outOfHoursGuardWeekStart(undefined)).toBe("mon");
    const policy: Policy = { id: "p", scope: { kind: "device" } };
    expect(outOfHoursGuardWeekStart(policy)).toBe("mon");
  });

  it("mirrors the budget clause's own weekStart when set", () => {
    const policy: Policy = {
      id: "p",
      scope: { kind: "device" },
      budget: { tz: "UTC", weekStart: "sun" },
    };
    expect(outOfHoursGuardWeekStart(policy)).toBe("sun");
  });
});

describe("outOfHoursContributions", () => {
  it("contributes nothing for a device with no STATUS at all", () => {
    expect(outOfHoursContributions(child(), {}, NOW_TUESDAY, TZ, "mon")).toEqual([]);
  });

  it("a paired, FRESH device contributes its own figures", () => {
    const rows = outOfHoursContributions(
      child(),
      { [PHONE_KEY]: status({ dayKey: "2026-08-04", outOfHoursWeekSecs: 8_100, outOfHoursNightsWeek: 3 }) },
      NOW_TUESDAY,
      TZ,
      "mon",
    );
    expect(rows).toEqual([{ deviceId: "d_phone", secs: 8_100, nights: 3 }]);
  });

  // C1 (review, 2026-08-04): a device whose last STATUS predates the ward's
  // CURRENT week must not go on contributing a week that has since rolled
  // over on the device's own ledger.
  it("a STALE device (last STATUS from a since-rolled-over week) contributes nothing", () => {
    const rows = outOfHoursContributions(
      child(),
      // Reported Sunday of the OLD week; a full week has since passed.
      { [PHONE_KEY]: status({ dayKey: "2026-08-09", outOfHoursWeekSecs: 8_100, outOfHoursNightsWeek: 3 }) },
      NOW_NEXT_TUESDAY,
      TZ,
      "mon",
    );
    expect(rows).toEqual([]);
  });

  // C1(b): a released device's last STATUS must not keep contributing.
  it("an UNPAIRED device contributes nothing even with a fresh dayKey", () => {
    const c = child({
      devices: [
        { id: "d_phone", label: "Old phone", platform: "android", pairing: "unpaired", devicePubkey: PHONE_KEY },
      ],
    });
    const rows = outOfHoursContributions(
      c,
      { [PHONE_KEY]: status({ dayKey: "2026-08-04", outOfHoursWeekSecs: 8_100, outOfHoursNightsWeek: 3 }) },
      NOW_TUESDAY,
      TZ,
      "mon",
    );
    expect(rows).toEqual([]);
  });

  it("treats absent secs/nights as zero rather than dropping the row", () => {
    const rows = outOfHoursContributions(
      child(),
      { [PHONE_KEY]: status({ dayKey: "2026-08-04" }) },
      NOW_TUESDAY,
      TZ,
      "mon",
    );
    expect(rows).toEqual([{ deviceId: "d_phone", secs: 0, nights: 0 }]);
  });

  it("keeps one device's fresh row and drops another's stale one, independently", () => {
    const c = child({
      devices: [
        { id: "d_phone", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE_KEY },
        { id: "d_laptop", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP_KEY },
      ],
    });
    const rows = outOfHoursContributions(
      c,
      {
        [PHONE_KEY]: status({ dayKey: "2026-08-04", outOfHoursWeekSecs: 600, outOfHoursNightsWeek: 1 }),
        [LAPTOP_KEY]: status({
          machine: LAPTOP_KEY,
          dayKey: "2026-07-20", // long stale
          outOfHoursWeekSecs: 9_000,
          outOfHoursNightsWeek: 5,
        }),
      },
      NOW_TUESDAY,
      TZ,
      "mon",
    );
    expect(rows).toEqual([{ deviceId: "d_phone", secs: 600, nights: 1 }]);
  });
});

describe("outOfHoursTotals", () => {
  it("sums seconds and takes the max of nights across devices", () => {
    const totals = outOfHoursTotals([
      { deviceId: "a", secs: 600, nights: 1 },
      { deviceId: "b", secs: 300, nights: 3 },
    ]);
    expect(totals).toEqual({ nights: 3, secs: 900 });
  });

  it("is zero for no contributions", () => {
    expect(outOfHoursTotals([])).toEqual({ nights: 0, secs: 0 });
  });

  // The whole point of the freshness guard: a stale device is filtered OUT of
  // the contributions list before this fold ever sees it, so it cannot pin
  // the max the way it could if it were folded in directly.
  it("a dropped (stale/unpaired) device cannot pin the max — it is simply absent from the fold", () => {
    const totals = outOfHoursTotals([{ deviceId: "fresh", secs: 0, nights: 0 }]);
    expect(totals).toEqual({ nights: 0, secs: 0 });
  });
});
