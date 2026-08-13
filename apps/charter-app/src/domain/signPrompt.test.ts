import { describe, expect, it } from "vitest";
import { signPrompt } from "./signPrompt";

describe("signPrompt", () => {
  // "Approve in Signet" is an instruction: go to that app and approve there.
  it("sends you to an external signer by name", () => {
    const p = signPrompt("signet", "Signet", "clause");
    expect(p.title).toBe("Confirm in Signet");
    expect(p.cta).toBe("Approve in Signet");
  });

  // …but the local key IS here. "Approve in This phone" told a parent to go
  // somewhere they already are, in broken English (decented, 2026-07-28).
  it("never tells you to go to the phone you are holding", () => {
    const p = signPrompt("local", "This phone", "clause");
    expect(p.title).toBe("Confirm this change");
    expect(p.cta).toBe("Approve");
    expect(p.title).not.toMatch(/in this phone/i);
    expect(p.cta).not.toMatch(/in this phone/i);
  });

  it("names what is being confirmed, not just 'change'", () => {
    expect(signPrompt("local", "This phone", "decision", "approved").title).toBe(
      "Confirm your decision",
    );
    expect(signPrompt("signet", "Signet", "decision", "approved").title).toBe("Confirm in Signet");
  });

  // The fallback label is already a phrase ("your approval app"), so it must
  // not be capitalised mid-sentence or double-prefixed.
  it("handles the unnamed-signer fallback", () => {
    const p = signPrompt("none", "your approval app", "clause");
    expect(p.cta).toBe("Approve in your approval app");
  });

  // 2026-08-04: "Approve" used to be the CTA no matter which button on the
  // request list raised the sheet — so denying a request meant pressing
  // "Approve". A local decision now skips the sheet entirely (see
  // mockSigner.ts / realSigner.ts), but an EXTERNAL signer still shows it,
  // and it must never say "Approve" for a no.
  describe("a denial never renders the word 'Approve'", () => {
    it("external signer, denied", () => {
      const p = signPrompt("signet", "Signet", "decision", "denied");
      expect(p.cta).toBe("Deny in Signet");
      expect(p.title).toBe("Confirm in Signet");
      expect(p.cta).not.toMatch(/approve/i);
    });

    it("external signer, approved — unchanged from today's wording", () => {
      const p = signPrompt("signet", "Signet", "decision", "approved");
      expect(p.cta).toBe("Approve in Signet");
    });

    // Defensive: the local signer's confirm gate is skipped for a decision in
    // production (see mockSigner.ts/realSigner.ts), so this combination never
    // actually reaches signPrompt — but the function itself must still be
    // correct if ever called this way.
    it("local signer, denied (defensive — this path is skipped in production)", () => {
      const p = signPrompt("local", "This phone", "decision", "denied");
      expect(p.cta).toBe("Deny");
      expect(p.cta).not.toMatch(/approve/i);
    });
  });

  // The whole point: "is this the local key" must be decided from the
  // signer's `kind`, not by pattern-matching its display label. A label is
  // product copy — fragile to compare against, and already broke once (see
  // the module header). Prove `kind` is what actually drives the branch by
  // using a label that does NOT say "This phone" at all.
  it("determines locality from signerKind, not signerLabel", () => {
    const p = signPrompt("local", "Some Renamed Label", "clause");
    expect(p.title).toBe("Confirm this change");
    expect(p.cta).toBe("Approve");
  });
});
