import { describe, expect, it } from "vitest";
import { lifelineToGrant } from "./clause";
import type { Lifeline } from "../domain/types";

const base: Lifeline = { numbers: [{ label: "Dad", number: "07700900123" }] };

describe("lifelineToGrant — torch", () => {
  it("carries the torch flag when the guardian enables it", () => {
    expect(lifelineToGrant({ ...base, torch: true }, 1700).torch).toBe(true);
  });

  /**
   * Absent, not `false`. A family who never touches this keeps a byte-identical
   * payload, and older wards (which have never heard of the field) are
   * unaffected — which is why this needs no ship-order guard, unlike the
   * 5-number change that older devices rejected wholesale.
   */
  it("omits it entirely when off, so pre-torch payloads are unchanged", () => {
    expect(lifelineToGrant(base, 1700)).toEqual({
      v: 1,
      numbers: [{ label: "Dad", number: "07700900123" }],
      issuedAt: 1700,
    });
    expect(lifelineToGrant({ ...base, torch: false }, 1700).torch).toBeUndefined();
  });

  it("travels alongside the other shade affordances", () => {
    const g = lifelineToGrant(
      {
        ...base,
        torch: true,
        emergencyServices: true,
        breakGlass: { enabled: true, scope: "full", durationMinutes: 10 },
      },
      1700,
    );
    expect(g.torch).toBe(true);
    expect(g.emergencyServices).toBe(true);
    expect(g.breakGlass?.enabled).toBe(true);
  });
});
