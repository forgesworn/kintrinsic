import { describe, expect, it } from "vitest";
import { householdForBackup } from "./householdExport";

describe("the household as a backup carries it", () => {
  it("leaves out the guardian PIN record still being sent, and keeps the rest", () => {
    const state = {
      children: [{ id: "c1" }],
      guardianKeys: {
        v: 1,
        pins: {
          a: { state: "sending", record: { v: 1, salt: "00", hash: "11" }, sentAt: 5 },
          b: { state: "set", sentAt: 4 },
        },
        installLabel: "Mum's phone",
      },
    };
    const out = JSON.parse(householdForBackup(JSON.stringify(state))!);
    expect(out.guardianKeys.pins).toEqual({ a: { state: "sending", sentAt: 5 }, b: { state: "set", sentAt: 4 } });
    expect(out.guardianKeys.installLabel).toBe("Mum's phone");
    expect(out.children).toEqual([{ id: "c1" }]);
    expect(JSON.stringify(out)).not.toContain('"hash"');
  });

  it("passes anything else through", () => {
    expect(householdForBackup(null)).toBeNull();
    expect(householdForBackup("not json")).toBe("not json");
    expect(householdForBackup(JSON.stringify({ children: [] }))).toBe(JSON.stringify({ children: [] }));
  });
});
