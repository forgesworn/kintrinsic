import { describe, expect, it } from "vitest";
import type { AppOpenDecisionWire, DecisionWire } from "../signer/Signer";
import { decisionPath } from "./decisionGate";

const WIRE: DecisionWire = {
  reqId: "a".repeat(64),
  nonce: "b".repeat(64),
  machine: "c".repeat(64),
  subject: "d".repeat(64),
  limitHit: "schedule",
};

const APP_OPEN_WIRE: AppOpenDecisionWire = {
  reqId: "a".repeat(64),
  nonce: "b".repeat(64),
  machine: "c".repeat(64),
  subject: "d".repeat(64),
  pkg: "com.mojang.minecraftpe",
};

describe("decisionPath", () => {
  it("signs when the signer is connected", () => {
    expect(decisionPath(true, WIRE)).toBe("sign");
    expect(decisionPath(true, undefined)).toBe("sign");
  });

  it("records locally when disconnected and the ask has no wire correlation (offline draft)", () => {
    expect(decisionPath(false, undefined)).toBe("record-locally");
  });

  it("REFUSES when disconnected and the ask is wire-correlated — never a silent fake approval", () => {
    expect(() => decisionPath(false, WIRE)).toThrowError(/parent approval/i);
  });

  // app.open asks are always real device asks (there's no simulated/demo
  // path for them), so an app.open decision must be gated exactly like a
  // time.extend one — never silently "record-locally" while disconnected.
  it("gates an app.open ask the same way — sign when connected, refuse when not", () => {
    expect(decisionPath(true, APP_OPEN_WIRE)).toBe("sign");
    expect(() => decisionPath(false, APP_OPEN_WIRE)).toThrowError(/parent approval/i);
  });
});
