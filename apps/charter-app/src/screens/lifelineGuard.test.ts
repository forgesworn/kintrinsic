import { describe, expect, it } from "vitest";
import { buildLifeline, lifelineNumberError } from "./Limits";

// The lifeline clause is only sent when at least one number survives the
// blank-row filter (the device drops a numberless body fail-closed). Every
// other lock-screen setting — torch, the emergency number, break-glass —
// rides that same clause, so a dirty lifeline with zero numbers is a change
// that can never reach the phone. The editor must say so and block the save
// instead of reporting success. (Found in the 2026-08-02 audit: a guardian
// who toggled break-glass off with no numbers configured saw "saved" while
// the device kept enforcing the old settings.)
describe("lifelineNumberError", () => {
  it("blocks a numberless lifeline so safety toggles cannot silently no-op", () => {
    const l = buildLifeline(undefined);
    l.torch = true;
    expect(lifelineNumberError(l)).toMatch(/add at least one number/i);
  });

  it("treats blank rows as no numbers", () => {
    const l = buildLifeline(undefined);
    l.numbers = [{ label: "  ", number: "" }];
    expect(lifelineNumberError(l)).toMatch(/add at least one number/i);
  });

  it("stays quiet once a real number exists", () => {
    const l = buildLifeline(undefined);
    l.numbers = [{ label: "Mum", number: "+44 7700 900123" }];
    expect(lifelineNumberError(l)).toBeNull();
  });

  it("still rejects more than five numbers", () => {
    const l = buildLifeline(undefined);
    l.numbers = Array.from({ length: 6 }, (_, i) => ({
      label: `G${i}`,
      number: `+44 7700 90012${i}`,
    }));
    expect(lifelineNumberError(l)).toMatch(/five/i);
  });
});
