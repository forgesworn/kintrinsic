import { describe, expect, it } from "vitest";
import type { Policy } from "./types";
import {
  SPLITTABLE_CONTROLS,
  effectivePolicyForDevice,
  isControlSplit,
  setControlShared,
  setControlSplit,
  setDeviceControl,
  pickOverrides,
} from "./effectivePolicy";

// Stand-in control values: this module only ever moves these objects around by
// identity, so their internal shape is irrelevant to what is under test.
const sched = { tz: "Europe/London", weekly: {} } as unknown as Policy["schedule"];
const budget = (mins: number) => ({ dailyMinutes: mins }) as unknown as Policy["budget"];
const web = { enabled: true } as unknown as Policy["web"];

const base: Policy = {
  id: "p1",
  scope: { kind: "device" },
  schedule: sched,
  budget: budget(120),
  web,
};

describe("effectivePolicyForDevice", () => {
  it("is the base charter when nothing is split", () => {
    expect(effectivePolicyForDevice(base, "laptop")).toEqual(base);
  });

  it("replaces only the split control, leaving the rest shared", () => {
    const policy: Policy = {
      ...base,
      deviceOverrides: { phone: { budget: budget(45) } },
    };
    const phone = effectivePolicyForDevice(policy, "phone");
    expect(phone.budget).toEqual(budget(45));
    // Untouched controls still come from the base.
    expect(phone.schedule).toEqual(base.schedule);
    expect(phone.web).toEqual(base.web);
  });

  it("gives an unlisted device the base charter untouched", () => {
    const policy: Policy = {
      ...base,
      deviceOverrides: { phone: { budget: budget(45) } },
    };
    expect(effectivePolicyForDevice(policy, "laptop").budget).toEqual(base.budget);
  });

  /**
   * The override map is Kintrinsic's own bookkeeping. If it ever reached the
   * wire, a device would receive the rules for its siblings — which is both a
   * privacy leak and a warden that could enforce the wrong charter.
   */
  it("never leaks the override map into the published policy", () => {
    const policy: Policy = {
      ...base,
      deviceOverrides: { phone: { budget: budget(45) } },
    };
    expect(effectivePolicyForDevice(policy, "phone").deviceOverrides).toBeUndefined();
    expect(effectivePolicyForDevice(policy, "laptop").deviceOverrides).toBeUndefined();
  });

  it("does not mutate the policy it is given", () => {
    const policy: Policy = {
      ...base,
      deviceOverrides: { phone: { budget: budget(45) } },
    };
    const snapshot = JSON.stringify(policy);
    effectivePolicyForDevice(policy, "phone");
    expect(JSON.stringify(policy)).toBe(snapshot);
  });
});

describe("splitting and re-sharing a control", () => {
  const devices = ["laptop", "phone"];

  /**
   * Splitting must be a no-op until the parent actually edits something:
   * every device inherits what it already had. Anything else silently
   * changes a child's rules the moment a parent taps "set separately".
   */
  it("seeds every device with the current shared value", () => {
    const split = setControlSplit(base, "budget", devices);
    expect(isControlSplit(split, "budget")).toBe(true);
    for (const d of devices) {
      expect(effectivePolicyForDevice(split, d).budget).toEqual(base.budget);
    }
  });

  it("edits one device without touching its sibling", () => {
    let p = setControlSplit(base, "budget", devices);
    p = setDeviceControl(p, "phone", "budget", budget(30));
    expect(effectivePolicyForDevice(p, "phone").budget).toEqual(budget(30));
    expect(effectivePolicyForDevice(p, "laptop").budget).toEqual(base.budget);
  });

  it("re-sharing drops every device's override for that control only", () => {
    let p = setControlSplit(base, "budget", devices);
    p = setControlSplit(p, "web", devices);
    p = setDeviceControl(p, "phone", "budget", budget(30));
    p = setControlShared(p, "budget");

    expect(isControlSplit(p, "budget")).toBe(false);
    expect(effectivePolicyForDevice(p, "phone").budget).toEqual(base.budget);
    // The other split control survives.
    expect(isControlSplit(p, "web")).toBe(true);
  });

  it("drops the override map entirely once the last control is re-shared", () => {
    let p = setControlSplit(base, "budget", devices);
    p = setControlShared(p, "budget");
    // Migration invariant: a never-split child stays byte-identical to today.
    expect(p.deviceOverrides).toBeUndefined();
    expect(p).toEqual(base);
  });

  it("only treats a control as split when a device actually carries it", () => {
    const empty: Policy = { ...base, deviceOverrides: { phone: {} } };
    expect(isControlSplit(empty, "budget")).toBe(false);
  });

  /**
   * A save signs only the changed dimensions. If an override for an UNCHANGED
   * control rode along, it would be grafted back on and emit a clause for a
   * dimension the parent never touched — re-signing a stale value with a fresh
   * issuedAt, which the device then treats as the newer truth.
   */
  it("narrows overrides to the controls actually being signed", () => {
    const overrides = {
      phone: { budget: budget(30), web },
      laptop: { web },
    };
    expect(pickOverrides(overrides, ["budget"])).toEqual({ phone: { budget: budget(30) } });
  });

  it("drops the map entirely when no signed control is split", () => {
    expect(pickOverrides({ phone: { web } }, ["budget"])).toBeUndefined();
    expect(pickOverrides(undefined, ["budget"])).toBeUndefined();
  });

  it("covers exactly the controls a family can sensibly split", () => {
    // Hotspot and lifeline are phone-only by nature and are not in the list.
    expect([...SPLITTABLE_CONTROLS]).toEqual([
      "schedule",
      "budget",
      "web",
      "apps",
      "learning",
    ]);
  });

  /**
   * N2 regression (2026-08 named-times review round 2): a save's `toSign`
   * Policy is built from `pickOverrides(deviceOverrides, controls)`, narrowed
   * to whatever dimensions that particular save actually touches. A prior fix
   * excluded "learning" from that `controls` list entirely (reasoning: no
   * editor creates a NEW per-device learning split any more), but a save that
   * sets the BASE `learning` clause happens on nearly every named-times save
   * — and with "learning" missing from `deviceOverrides`, a device that
   * legitimately split it stopped receiving its own rule at all:
   * `effectivePolicyForDevice` fell through to the base value, silently
   * replacing whatever that device's own learning override said. Mirrors
   * `signClause`'s real composition (`toSign` + `effectivePolicyForDevice`
   * per device), not just `pickOverrides` in isolation, because the bug only
   * shows up one layer up from where the exclusion lived.
   */
  it("a device's own learning override survives a save that also sets the base learning clause", () => {
    const learningBase = { enabled: true, apps: [] } as unknown as Policy["learning"];
    const learningPhone = {
      enabled: true,
      apps: [{ id: "scratch", label: "Scratch", kind: "native", exec: "scratch" }],
    } as unknown as Policy["learning"];
    const saved: Policy = {
      ...base,
      learning: undefined,
      deviceOverrides: { phone: { learning: learningPhone } },
    };
    // What store.tsx's savePolicy builds as `toSign` for a named-times save
    // that touches the base learning clause (`next.learning !== undefined`):
    // "learning" MUST stay among the controls `pickOverrides` narrows to.
    const toSign: Policy = {
      ...saved,
      learning: learningBase,
      deviceOverrides: pickOverrides(saved.deviceOverrides, [...SPLITTABLE_CONTROLS]),
    };
    const effective = effectivePolicyForDevice(toSign, "phone");
    expect(effective.learning).toEqual(learningPhone);
    expect(effective.learning).not.toEqual(learningBase);
    // An unsplit device correctly gets the new base value.
    expect(effectivePolicyForDevice(toSign, "laptop").learning).toEqual(learningBase);
  });
});
