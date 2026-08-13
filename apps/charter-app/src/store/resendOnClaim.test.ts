import { describe, expect, it } from "vitest";
import type { Child } from "../domain/types";
import { readyResends, type PendingResend } from "./resendOnClaim";

const M1 = "a".repeat(64);

function child(devicePubkey?: string): Child {
  return {
    id: "child_mia",
    name: "Mia",
    color: "#fff",
    dependantPubkey: null,
    devices: devicePubkey
      ? [
          {
            id: "dev1",
            label: "Mia’s phone",
            platform: "android",
            pairing: "paired",
            devicePubkey,
          },
        ]
      : [],
    policies: [],
  };
}

describe("readyResends", () => {
  // A claim dispatches state; the claimed device only exists in COMMITTED
  // state one render later. Resending before then resolves a child target
  // that misses the new phone — the exact device the resend exists for.
  it("holds a resend until the claimed device is committed", () => {
    const pending: PendingResend[] = [{ childId: "child_mia", machine: M1 }];
    expect(readyResends(pending, [child(undefined)])).toEqual([]);
    expect(readyResends(pending, [child(M1)])).toEqual(pending);
  });

  it("only releases the claim it is holding for", () => {
    const pending: PendingResend[] = [{ childId: "child_mia", machine: M1 }];
    // A different device landing must not trigger the resend early.
    expect(readyResends(pending, [child("b".repeat(64))])).toEqual([]);
    // Nor a matching device on a different child.
    const sibling = { ...child(M1), id: "child_two" };
    expect(readyResends(pending, [sibling])).toEqual([]);
  });
});
