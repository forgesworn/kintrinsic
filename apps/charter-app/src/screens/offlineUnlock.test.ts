import { describe, it, expect, vi } from "vitest";
import { getPublicKey } from "nostr-tools";
import { unlockCodeForDevice } from "../unlock";
import {
  computeUnlockView,
  formatUnlockCode,
  isChallengeComplete,
  isValidDevicePubkey,
  sanitizeChallenge,
  UNLOCK_CHALLENGE_LEN,
} from "./offlineUnlock";

// A fixed guardian key + a real device pubkey derived from another fixed key —
// so the "displayed code == unlockCodeForDevice(...)" assertion is meaningful.
function key(byte: number): Uint8Array {
  return new Uint8Array(32).fill(byte);
}
const GUARDIAN_SK = key(0x07);
const DEVICE_PK = getPublicKey(key(0x09)); // 64 lowercase hex

describe("sanitizeChallenge", () => {
  it("uppercases lowercase input", () => {
    expect(sanitizeChallenge("k7qn")).toBe("K7QN");
  });

  it("drops illegal characters (spaces, punctuation, ambiguous 0/1/I/O)", () => {
    // The excluded chars are 0, 1, I, O (L IS in the alphabet), plus anything
    // non-alphanumeric like spaces/dashes/punctuation.
    expect(sanitizeChallenge("k 7-q!n")).toBe("K7QN");
    expect(sanitizeChallenge("OI10")).toBe(""); // O, I, 1, 0 all excluded
    expect(sanitizeChallenge("aO1b")).toBe("AB"); // O and 1 dropped, A/B kept
    expect(sanitizeChallenge("l")).toBe("L"); // L is a valid character
  });

  it("caps at the challenge length", () => {
    expect(sanitizeChallenge("ABCDEFG")).toBe("ABCD");
    expect(sanitizeChallenge("ABCD").length).toBe(UNLOCK_CHALLENGE_LEN);
  });

  it("is idempotent (safe as a controlled-input value)", () => {
    const once = sanitizeChallenge("k 7-q!nXYZ");
    expect(sanitizeChallenge(once)).toBe(once);
  });
});

describe("isChallengeComplete / isValidDevicePubkey / formatUnlockCode", () => {
  it("challenge is complete only at full length", () => {
    expect(isChallengeComplete("K7Q")).toBe(false);
    expect(isChallengeComplete("K7QN")).toBe(true);
  });

  it("accepts only 64 lowercase hex as a device pubkey", () => {
    expect(isValidDevicePubkey(DEVICE_PK)).toBe(true);
    expect(isValidDevicePubkey(undefined)).toBe(false);
    expect(isValidDevicePubkey(null)).toBe(false);
    expect(isValidDevicePubkey("abc")).toBe(false);
    expect(isValidDevicePubkey(DEVICE_PK.toUpperCase())).toBe(false); // not lowercase
  });

  it("groups the 8-digit code as 1234 5678", () => {
    expect(formatUnlockCode("12345678")).toBe("1234 5678");
    expect(formatUnlockCode("123")).toBe("123"); // leaves non-8-digit alone
  });
});

describe("computeUnlockView", () => {
  it("shows the real crypto code once the challenge is complete", () => {
    const loadGuardianSk = vi.fn(() => GUARDIAN_SK);
    const view = computeUnlockView({
      signerKind: "local",
      devicePubkey: DEVICE_PK,
      rawChallenge: "k7qn",
      loadGuardianSk,
    });
    expect(view.kind).toBe("code");
    if (view.kind !== "code") throw new Error("unreachable");
    expect(view.challenge).toBe("K7QN");
    // The wiring must call the shared crypto with the SAME inputs — no reimpl.
    expect(view.code).toBe(unlockCodeForDevice(GUARDIAN_SK, DEVICE_PK, "K7QN"));
    expect(loadGuardianSk).toHaveBeenCalledTimes(1);
  });

  it("prompts to finish (and never touches the key) while incomplete", () => {
    const loadGuardianSk = vi.fn(() => GUARDIAN_SK);
    const view = computeUnlockView({
      signerKind: "local",
      devicePubkey: DEVICE_PK,
      rawChallenge: "k7",
      loadGuardianSk,
    });
    expect(view.kind).toBe("incomplete");
    expect(view.challenge).toBe("K7");
    expect(loadGuardianSk).not.toHaveBeenCalled();
  });

  it("shows the not-local fallback and never reads the key on a bunker signer", () => {
    const loadGuardianSk = vi.fn(() => GUARDIAN_SK);
    for (const signerKind of ["signet", "heartwood", "none"] as const) {
      const view = computeUnlockView({
        signerKind,
        devicePubkey: DEVICE_PK,
        rawChallenge: "K7QN", // complete — proving the gate is the signer, not input
        loadGuardianSk,
      });
      expect(view.kind).toBe("not-local");
    }
    expect(loadGuardianSk).not.toHaveBeenCalled();
  });

  it("shows the no-pubkey fallback for an unpaired/keyless device", () => {
    const loadGuardianSk = vi.fn(() => GUARDIAN_SK);
    const view = computeUnlockView({
      signerKind: "local",
      devicePubkey: null,
      rawChallenge: "K7QN",
      loadGuardianSk,
    });
    expect(view.kind).toBe("no-pubkey");
    expect(loadGuardianSk).not.toHaveBeenCalled();
  });

  it("fails safe (no code) if reading the guardian key throws", () => {
    const view = computeUnlockView({
      signerKind: "local",
      devicePubkey: DEVICE_PK,
      rawChallenge: "K7QN",
      loadGuardianSk: () => {
        throw new Error("corrupt");
      },
    });
    expect(view.kind).toBe("error");
  });
});
