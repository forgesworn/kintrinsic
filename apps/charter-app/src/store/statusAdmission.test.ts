import { describe, expect, it } from "vitest";
import { admitStatus } from "./statusAdmission";

const PHONE = "aa".repeat(32);
const family = [{ devices: [{ id: "d1", devicePubkey: PHONE }, { id: "d2", devicePubkey: null }] }] as never;

describe("admitStatus", () => {
  it("fully admits a device a child claims", () => {
    expect(admitStatus({ machine: PHONE }, family, new Set())).toBe("claimed");
  });

  it("admits an unclaimed machine only while it echoes a token we minted", () => {
    const tokens = new Set(["tok-1"]);
    expect(admitStatus({ machine: "bb".repeat(32), pairToken: "tok-1" }, family, tokens)).toBe("pairing");
    expect(admitStatus({ machine: "bb".repeat(32), pairToken: "guess" }, family, tokens)).toBe("stranger");
  });

  it("drops a stranger — an authentic wrap is not proof of pairing", () => {
    expect(admitStatus({ machine: "cc".repeat(32) }, family, new Set(["tok-1"]))).toBe("stranger");
    expect(admitStatus({ machine: "cc".repeat(32), pairToken: null }, [], new Set())).toBe("stranger");
  });
});
