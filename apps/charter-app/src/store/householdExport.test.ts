import { describe, expect, it } from "vitest";
import { backupWorkingKeys, householdForBackup, withRestoreNote } from "./householdExport";

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

describe("working keys and the restore note", () => {
  const R = "11".repeat(32);
  const W = "22".repeat(32);
  const LOST = "33".repeat(32);
  const OTHER = "44".repeat(32);

  it("lists this install's W with its label and the identity's other keys, never the identity or another parent's", () => {
    expect(
      backupWorkingKeys({
        root: R,
        workingPubkey: W,
        installLabel: "Mum's phone",
        reported: [
          { root: R, keys: [LOST, W] },
          { root: OTHER, keys: ["55".repeat(32)] },
        ],
      }),
    ).toEqual([{ pubkey: W, label: "Mum's phone" }, { pubkey: LOST }]);
    // A phone-held identity that never minted W: its key is the identity, not listed.
    expect(backupWorkingKeys({ root: R, workingPubkey: R, reported: [] })).toEqual([]);
  });

  it("records the restore in the household", () => {
    const out = JSON.parse(withRestoreNote(JSON.stringify({ children: [], guardianKeys: { v: 1, installLabel: "x" } }), [{ pubkey: LOST }], 7));
    expect(out.guardianKeys).toEqual({ v: 1, installLabel: "x", restoredFrom: { at: 7, workingKeys: [{ pubkey: LOST }] } });
  });
});
