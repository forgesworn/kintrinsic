import { describe, expect, it } from "vitest";
import type { ClausePayload, GrantGift } from "./types";

// GrantGift's groupId (named times: "give more Play time" without being
// asked). No pure `giftToGrant` helper exists yet — the body is built inline
// where the guardian signs it (`signer/realSigner.ts`, Task 12's territory) —
// so this pins the WIRE SHAPE itself: a groupId set round-trips through JSON
// untouched, and an absent one is genuinely ABSENT on the wire (never a null),
// which is what lets an old ward's "the body doesn't deny unknown keys" fail-
// generous behaviour work (spec/contract.md "Gifted time").

const base = (over: Partial<GrantGift> = {}): GrantGift => ({
  v: 1,
  issuedAt: 1700,
  id: "1700-abcd",
  minutes: 20,
  expiresAt: 1800,
  ...over,
});

describe("GrantGift.groupId", () => {
  it("round-trips through JSON when present", () => {
    const g = base({ groupId: "play" });
    const back = JSON.parse(JSON.stringify(g)) as GrantGift;
    expect(back).toEqual(g);
    expect(back.groupId).toBe("play");
  });

  it("is genuinely absent from the wire (not null) when unset — old-ward errs generous", () => {
    const g = base();
    const json = JSON.stringify(g);
    expect(json).not.toContain("groupId");
    const back = JSON.parse(json) as GrantGift;
    expect(back.groupId).toBeUndefined();
  });

  it("a gift clause carrying a groupId still typechecks as a ClausePayload", () => {
    const clause: ClausePayload = {
      v: 1,
      kind: "gift",
      issuedAt: 1700,
      body: base({ groupId: "play" }),
    };
    expect((clause.body as GrantGift).groupId).toBe("play");
  });
});
