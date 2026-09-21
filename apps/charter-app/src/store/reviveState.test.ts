import { describe, expect, it } from "vitest";
import { reviveState } from "./reviveState";

const empty = {
  children: [],
  requests: [],
  activity: [],
  signer: { connected: false, kind: "none", autoSign: false },
};

describe("reviveState", () => {
  it("absent storage is a clean start, not damage", () => {
    expect(reviveState(null, empty)).toEqual({ kind: "absent" });
    expect(reviveState("", empty)).toEqual({ kind: "absent" });
  });

  it("a write cut short is BROKEN — handed back to be set aside, never an empty family", () => {
    const raw = '{"children":[{"id":"c1","name":"Rob';
    expect(reviveState(raw, empty)).toEqual({ kind: "broken", raw });
  });

  it("the wrong shape is broken too, not a white screen on the first action", () => {
    for (const raw of ["{}", "[]", "null", '"x"', '{"children":{}}']) {
      expect(reviveState(raw, empty).kind, raw).toBe("broken");
    }
  });

  it("keeps the children and repairs the rest", () => {
    const raw = JSON.stringify({ children: [{ id: "c1" }], requests: "junk", signer: { autoSign: true } });
    const r = reviveState(raw, empty);
    expect(r).toEqual({
      kind: "ok",
      state: {
        children: [{ id: "c1" }],
        requests: [],
        activity: [],
        signer: { connected: false, kind: "none", autoSign: true },
      },
    });
  });

  it("passes a whole state through untouched", () => {
    const whole = { ...empty, children: [{ id: "c1" }], activity: [{ id: "a" }] };
    expect(reviveState(JSON.stringify(whole), empty)).toEqual({ kind: "ok", state: whole });
  });
});
