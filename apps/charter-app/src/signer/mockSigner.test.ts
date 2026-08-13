import { describe, expect, it, vi } from "vitest";
import { MockSigner, SignerCancelled, type ConfirmGate } from "./mockSigner";

function makeSigner(confirmGate?: ConfirmGate) {
  return new MockSigner({ confirmGate, delayMs: 0 });
}

const DEVICE_POLICY = { id: "p1", scope: { kind: "device" as const } };

// 2026-08-04: pressing Approve or Not now on a request card used to route
// through the same confirm gate as a rule edit, whose CTA always read
// "Approve" — so denying a child's request meant pressing "Approve". The
// fix: a decision signed by the LOCAL key skips the gate entirely (the list
// press IS the confirmation); an external signer still gates it, but with
// context rich enough that the sheet can say "Deny" for a no
// (see domain/signPrompt.test.ts). `action: "clause"` is untouched either way.
describe("MockSigner.authorize — decision vs clause gating", () => {
  describe("a decision on the LOCAL signer skips the confirm gate entirely", () => {
    it("approve never calls the gate", async () => {
      const gate = vi.fn(() => true);
      const s = makeSigner(gate);
      await s.connect("local");
      await expect(s.signDecision("req_1", "approved")).resolves.toEqual({ ok: true });
      expect(gate).not.toHaveBeenCalled();
    });

    it("deny never calls the gate", async () => {
      const gate = vi.fn(() => true);
      const s = makeSigner(gate);
      await s.connect("local");
      await expect(s.signDecision("req_1", "denied")).resolves.toEqual({ ok: true });
      expect(gate).not.toHaveBeenCalled();
    });

    it("a gate wired to refuse has no effect — it is never asked", async () => {
      const gate = vi.fn(() => false);
      const s = makeSigner(gate);
      await s.connect("local");
      // If the gate were still consulted this would reject with SignerCancelled.
      await expect(s.signDecision("req_1", "denied")).resolves.toEqual({ ok: true });
      expect(gate).not.toHaveBeenCalled();
    });
  });

  describe("an external signer still gates a decision, with the right context", () => {
    it("calls the gate carrying action, decision, and signerKind", async () => {
      const gate = vi.fn(() => true);
      const s = makeSigner(gate);
      await s.connect("signet");
      await s.signDecision("req_1", "denied");
      expect(gate).toHaveBeenCalledWith(
        expect.objectContaining({
          action: "decision",
          decision: "denied",
          signerKind: "signet",
          signerLabel: "Signet",
        }),
      );
    });

    it("a cancelled gate still blocks an external decision", async () => {
      const s = makeSigner(() => false);
      await s.connect("signet");
      await expect(s.signDecision("req_1", "approved")).rejects.toBeInstanceOf(SignerCancelled);
    });
  });

  describe("action: 'clause' is completely unchanged", () => {
    it("still gates a clause save on the local signer — the decision skip does not leak", async () => {
      const gate = vi.fn(() => true);
      const s = makeSigner(gate);
      await s.connect("local");
      await s.signClause("child_1", DEVICE_POLICY);
      expect(gate).toHaveBeenCalledWith(
        expect.objectContaining({ action: "clause", decision: undefined, signerKind: "local" }),
      );
    });

    it("a cancelled gate still blocks a clause save on the local signer", async () => {
      const s = makeSigner(() => false);
      await s.connect("local");
      await expect(s.signClause("child_1", DEVICE_POLICY)).rejects.toBeInstanceOf(
        SignerCancelled,
      );
    });
  });

  it("autoSign skips the gate regardless of action or kind (unchanged)", async () => {
    const gate = vi.fn(() => true);
    const s = makeSigner(gate);
    await s.connect("signet");
    await s.setAutoSign(true);
    await s.signDecision("req_1", "denied");
    await s.signClause("child_1", DEVICE_POLICY);
    expect(gate).not.toHaveBeenCalled();
  });
});
