import { describe, expect, it } from "vitest";
import { carrierShellState } from "./bridge";

// Why this exists: on 2026-08-06 decented updated the PWA and the ward app and
// still saw "Your ward" on a break-glass notification. The naming lives in the
// carrier shell's Kotlin, which only a new APK carries — and pushing a roster
// to an older shell was a SILENT no-op, so nothing could tell him. Silence is
// the wrong answer; these pin the signal that replaced it.

describe("carrierShellState", () => {
  it("says nothing when the page is not inside the carrier at all", () => {
    // An ordinary browser tab: there is no shell to be stale.
    expect(carrierShellState(undefined)).toBe("not-carrier");
  });

  it("flags a shell that cannot take a roster as needing an update", () => {
    // The pre-2026-08-04 APK: provision/isCarrier only, no `roster`.
    expect(carrierShellState({})).toBe("needs-update");
  });

  it("is satisfied once the shell exposes the roster hand-off", () => {
    expect(carrierShellState({ roster: () => {} })).toBe("current");
  });

  /**
   * Feature-detected, never version-compared — the page has no reliable read
   * on the shell's versionCode, and "does it expose the method I need" is the
   * honest question anyway. A future shell that keeps `roster` stays current
   * without this test needing to know its number.
   */
  it("does not care what version the shell claims to be", () => {
    expect(carrierShellState({ roster: () => {}, version: 99 } as never)).toBe("current");
    expect(carrierShellState({ version: 99 } as never)).toBe("needs-update");
  });
});
