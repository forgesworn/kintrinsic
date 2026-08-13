import { describe, expect, it } from "vitest";
import {
  unrecognisedGuardTz,
  unrecognisedLine,
  unrecognisedRows,
  UNRECOGNISED_EXPLAINER,
} from "./unrecognisedTime";
import { resolveChildTz } from "./childTz";
import type { Child, Policy } from "./types";
import type { DeviceStatus } from "../wire/status";

const LAPTOP_KEY = "a".repeat(64);
const PHONE_KEY = "c".repeat(64);

function child(over: Partial<Child> = {}): Child {
  return {
    id: "child_sam",
    name: "Sam",
    color: "#000",
    dependantPubkey: null,
    devices: [
      { id: "d_laptop", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP_KEY },
    ],
    policies: [],
    ...over,
  };
}

function status(over: Partial<DeviceStatus> = {}): DeviceStatus {
  return {
    v: 1,
    subject: "b".repeat(64),
    machine: LAPTOP_KEY,
    ts: 1_700_000_000,
    dayKey: "2026-08-03",
    usedTodaySecs: 0,
    windowLeftSecs: 0,
    quotaLeftSecs: 0,
    effectiveSecs: 0,
    locked: false,
    source: "guardian",
    ...over,
  };
}

// Fixed, timezone-pinned "now" so the day-guard tests (F5) are deterministic
// regardless of the machine running them — UTC noon on the same calendar day
// every `status()` fixture's `dayKey` claims.
const NOW_SAME_DAY = Math.floor(Date.parse("2026-08-03T12:00:00Z") / 1000);
const NOW_NEXT_DAY = Math.floor(Date.parse("2026-08-04T12:00:00Z") / 1000);
const TZ = "UTC";

describe("unrecognisedRows", () => {
  it("is empty when the field is absent — never a finding from silence", () => {
    const rows = unrecognisedRows(child(), { [LAPTOP_KEY]: status() }, NOW_SAME_DAY, TZ);
    expect(rows).toEqual([]);
  });

  it("is empty when the device genuinely reported zero", () => {
    const rows = unrecognisedRows(
      child(),
      { [LAPTOP_KEY]: status({ unrecognisedTodaySecs: 0 }) },
      NOW_SAME_DAY,
      TZ,
    );
    expect(rows).toEqual([]);
  });

  it("is empty for a device that has never reported at all", () => {
    expect(unrecognisedRows(child(), {}, NOW_SAME_DAY, TZ)).toEqual([]);
  });

  it("yields one row for a device reporting > 0 on the ward's current day", () => {
    const rows = unrecognisedRows(
      child(),
      { [LAPTOP_KEY]: status({ unrecognisedTodaySecs: 11_520 }) },
      NOW_SAME_DAY,
      TZ,
    );
    expect(rows).toEqual([{ deviceId: "d_laptop", deviceLabel: "Sam's laptop", secs: 11_520 }]);
  });

  it("ignores an unpaired or keyless device", () => {
    const c = child({
      devices: [
        { id: "d1", label: "Old laptop", platform: "linux", pairing: "unpaired", devicePubkey: LAPTOP_KEY },
      ],
    });
    expect(
      unrecognisedRows(c, { [LAPTOP_KEY]: status({ unrecognisedTodaySecs: 500 }) }, NOW_SAME_DAY, TZ),
    ).toEqual([]);
  });

  it("gives one row per device when more than one has something unrecognised", () => {
    const c = child({
      devices: [
        { id: "d_laptop", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP_KEY },
        { id: "d_phone", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE_KEY },
      ],
    });
    const rows = unrecognisedRows(
      c,
      {
        [LAPTOP_KEY]: status({ unrecognisedTodaySecs: 600 }),
        [PHONE_KEY]: status({ machine: PHONE_KEY, unrecognisedTodaySecs: 300 }),
      },
      NOW_SAME_DAY,
      TZ,
    );
    expect(rows.map((r) => r.deviceId).sort()).toEqual(["d_laptop", "d_phone"]);
  });

  // F5 (review, 2026-08-03): a guardian with the PWA open across the ward's
  // midnight must not keep reading yesterday's figure inside a "Today"-shaped
  // card once the device's OWN dayKey no longer matches the ward's current
  // day — even though the number itself is still > 0.
  describe("day guard (F5)", () => {
    it("drops a row whose dayKey is not the ward's CURRENT day", () => {
      const rows = unrecognisedRows(
        child(),
        { [LAPTOP_KEY]: status({ dayKey: "2026-08-03", unrecognisedTodaySecs: 11_520 }) },
        NOW_NEXT_DAY,
        TZ,
      );
      expect(rows).toEqual([]);
    });

    it("keeps a row whose dayKey matches the ward's current day", () => {
      const rows = unrecognisedRows(
        child(),
        { [LAPTOP_KEY]: status({ dayKey: "2026-08-03", unrecognisedTodaySecs: 11_520 }) },
        NOW_SAME_DAY,
        TZ,
      );
      expect(rows).toHaveLength(1);
    });

    it("keeps one device's fresh row and drops another's stale one, independently", () => {
      const c = child({
        devices: [
          { id: "d_laptop", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP_KEY },
          { id: "d_phone", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE_KEY },
        ],
      });
      const rows = unrecognisedRows(
        c,
        {
          // Laptop reported TODAY (still current).
          [LAPTOP_KEY]: status({ dayKey: "2026-08-04", unrecognisedTodaySecs: 400 }),
          // Phone's last report was YESTERDAY — stale across the rollover.
          [PHONE_KEY]: status({ machine: PHONE_KEY, dayKey: "2026-08-03", unrecognisedTodaySecs: 900 }),
        },
        NOW_NEXT_DAY,
        TZ,
      );
      expect(rows.map((r) => r.deviceId)).toEqual(["d_laptop"]);
    });
  });

  // New-1 (review, 2026-08-03): the day guard must mirror charterd's OWN
  // `enforcement_tz_of` (schedule, then budget, then UTC) — never
  // `resolveChildTz`, which also falls back to buckets.tz. They disagree for
  // exactly this feature's own minimal config: a named-times-only ward
  // (buckets, no schedule/budget) — the device stamps `dayKey` in UTC, but
  // `resolveChildTz` would return the buckets tz. Robin's laptop (the
  // hardware round's ward) is exactly this shape.
  describe("tz source for a buckets-only ward (New-1)", () => {
    const bucketsOnlyPolicy: Policy = {
      id: "pol",
      scope: { kind: "device" },
      buckets: { enabled: true, tz: "Europe/London", buckets: [] },
    };
    // 23:30 UTC on 2026-08-03 is already 00:30 BST on 2026-08-04 in London —
    // a different calendar day. The device (UTC) stamps today's report
    // "2026-08-03"; only the CORRECT guard tz reads that as still current.
    const NOW_LONDON_ROLLED_OVER = Math.floor(Date.parse("2026-08-03T23:30:00Z") / 1000);

    it("unrecognisedGuardTz never falls back to buckets.tz, unlike resolveChildTz", () => {
      expect(unrecognisedGuardTz(bucketsOnlyPolicy)).toBe("UTC");
      // The contrast is the whole bug: resolveChildTz is CORRECT for its own
      // purpose (a grant's expiry) and WRONG for this one.
      expect(resolveChildTz(bucketsOnlyPolicy)).toBe("Europe/London");
    });

    it("keeps a fresh row across the ward's UTC offset window using the correct tz", () => {
      const rows = unrecognisedRows(
        child(),
        { [LAPTOP_KEY]: status({ dayKey: "2026-08-03", unrecognisedTodaySecs: 7_200 }) },
        NOW_LONDON_ROLLED_OVER,
        unrecognisedGuardTz(bucketsOnlyPolicy),
      );
      expect(rows).toHaveLength(1);
    });

    it("demonstrates the bug this fixes: the WRONG (buckets/resolveChildTz) tz would drop the same row", () => {
      const rows = unrecognisedRows(
        child(),
        { [LAPTOP_KEY]: status({ dayKey: "2026-08-03", unrecognisedTodaySecs: 7_200 }) },
        NOW_LONDON_ROLLED_OVER,
        resolveChildTz(bucketsOnlyPolicy) as string,
      );
      expect(rows).toEqual([]);
    });
  });
});

describe("unrecognisedLine", () => {
  it("reads plainly with no device name for a single-device ward", () => {
    const line = unrecognisedLine({ deviceId: "d_laptop", deviceLabel: "Sam's laptop", secs: 11_520 }, false);
    expect(line).toBe("3h 12m Kintrinsic didn't recognise");
  });

  it("names the device when the ward has more than one", () => {
    const line = unrecognisedLine({ deviceId: "d_laptop", deviceLabel: "Sam's laptop", secs: 11_520 }, true);
    expect(line).toBe("3h 12m Kintrinsic didn't recognise on Sam's laptop");
  });

  it("formats under an hour as plain minutes", () => {
    expect(unrecognisedLine({ deviceId: "d1", deviceLabel: "X", secs: 300 }, false)).toBe(
      "5m Kintrinsic didn't recognise",
    );
  });

  it("never reads 0m — a sub-minute positive value rounds up to 1m", () => {
    expect(unrecognisedLine({ deviceId: "d1", deviceLabel: "X", secs: 10 }, false)).toBe(
      "1m Kintrinsic didn't recognise",
    );
  });
});

describe("UNRECOGNISED_EXPLAINER", () => {
  it("never claims completeness", () => {
    expect(UNRECOGNISED_EXPLAINER.toLowerCase()).not.toMatch(/everything they (did|used|ran)/);
    expect(UNRECOGNISED_EXPLAINER).toMatch(/floor/);
  });

  // F7 (review, 2026-08-03): the mechanism alone offers no innocent reading,
  // and on Approvals it sits directly above a pending ask — the worst place
  // to read as evidence rather than context (contract's own gloss: "grounds
  // for a conversation, not a dossier").
  it("offers a benign reading, not just the mechanism", () => {
    expect(UNRECOGNISED_EXPLAINER.toLowerCase()).toMatch(/nothing surprising|worth asking/);
  });

  // Follow-up polish: the benign reading shouldn't re-assert the AGENCY F3
  // removed from the userInstalled mark ("installed by" -> "from their
  // account") — it doesn't know WHO installed anything either.
  it("does not claim the ward installed it themselves", () => {
    expect(UNRECOGNISED_EXPLAINER.toLowerCase()).not.toMatch(/installed themselves/);
  });

  it("keeps the floor-not-a-total sentence exactly as it was", () => {
    expect(UNRECOGNISED_EXPLAINER).toContain(
      "Kintrinsic can't see everything, so this is a floor, not the whole picture.",
    );
  });
});
