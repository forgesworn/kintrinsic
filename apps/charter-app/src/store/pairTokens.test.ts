import { beforeEach, describe, expect, it } from "vitest";
import {
  forgetPairToken,
  livePairTokens,
  mintedPairTokens,
  PAIR_TOKEN_MEMORY_MS,
  rememberPairToken,
} from "./pairTokens";

const T0 = 1_700_000_000_000;
const A = "0123456789abcdef0123456789abcdef";
const B = "fedcba9876543210fedcba9876543210";

describe("pairTokens", () => {
  beforeEach(() => localStorage.clear());

  it("remembers a token it just minted", () => {
    rememberPairToken(A, "child_mia", T0);
    expect(livePairTokens(T0).has(A)).toBe(true);
    expect(mintedPairTokens(T0)[0].childId).toBe("child_mia");
  });

  // The whole reason this is not just in-session state: the split-brain
  // recovery only matters once the app has been closed and reopened.
  it("survives a reload", () => {
    rememberPairToken(A, "child_mia", T0);
    // A fresh read of the same backing store is what a reload amounts to.
    expect(livePairTokens(T0 + 60_000).has(A)).toBe(true);
  });

  it("keeps several outstanding tokens — a parent may set up two phones", () => {
    rememberPairToken(A, "child_mia", T0);
    rememberPairToken(B, "child_rook", T0 + 1000);
    expect(livePairTokens(T0 + 2000)).toEqual(new Set([A, B]));
  });

  it("newest first", () => {
    rememberPairToken(A, "child_mia", T0);
    rememberPairToken(B, "child_rook", T0 + 1000);
    expect(mintedPairTokens(T0 + 2000).map((t) => t.token)).toEqual([B, A]);
  });

  it("ages a token out of the ledger", () => {
    rememberPairToken(A, "child_mia", T0);
    expect(livePairTokens(T0 + PAIR_TOKEN_MEMORY_MS).has(A)).toBe(true);
    expect(livePairTokens(T0 + PAIR_TOKEN_MEMORY_MS + 1).has(A)).toBe(false);
  });

  it("spends a token once its device is recorded", () => {
    rememberPairToken(A, "child_mia", T0);
    forgetPairToken(A, T0);
    expect(livePairTokens(T0).has(A)).toBe(false);
  });

  it("re-minting the same token does not double it up", () => {
    rememberPairToken(A, "child_mia", T0);
    rememberPairToken(A, "child_mia", T0 + 1000);
    expect(mintedPairTokens(T0 + 2000)).toHaveLength(1);
  });

  it("caps the ledger so a mint loop cannot grow it forever", () => {
    for (let i = 0; i < 100; i++) rememberPairToken(`t${i}`, "child_mia", T0 + i);
    expect(mintedPairTokens(T0 + 200).length).toBeLessThanOrEqual(32);
  });

  // Fail CLOSED: an unreadable ledger vouches for NOTHING, so the claim banner
  // simply doesn't appear. The way forward is showing the pairing code again.
  it("treats a corrupt ledger as empty rather than trusting it", () => {
    localStorage.setItem("charter.pair.tokens.v1", "{not json");
    expect(livePairTokens(T0).size).toBe(0);
    localStorage.setItem("charter.pair.tokens.v1", JSON.stringify(["a", 2, null, { token: 7 }]));
    expect(livePairTokens(T0).size).toBe(0);
  });

  // A clock that jumped backwards must not turn a stale token live again.
  it("ignores a token minted in the future", () => {
    rememberPairToken(A, "child_mia", T0 + 60_000);
    expect(livePairTokens(T0).has(A)).toBe(false);
  });
});
