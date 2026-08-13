import { describe, it, expect } from "vitest";
import { getPublicKey, nip44 } from "nostr-tools";
import { unlockCode, unlockCodeForDevice, UNLOCK_CODE_DIGITS } from "./unlock";

function key(byte: number): Uint8Array {
  return new Uint8Array(32).fill(byte);
}

describe("unlockCode parity with charter-crypto (Rust)", () => {
  it("matches the shared known-answer vectors", () => {
    // These MUST equal the Rust `known_answer_vectors_match_ts_mirror` outputs.
    expect(unlockCode(key(0x07), "K7QN")).toBe("48437822");
    expect(unlockCode(key(0x07), "4821")).toBe("55118120");
    expect(unlockCode(key(0x2a), "K7QN")).toBe("92990140");
    const k = new Uint8Array(32);
    for (let i = 0; i < 32; i++) k[i] = i;
    expect(unlockCode(k, "HELLO")).toBe("27854096");
  });

  it("is exactly 8 ascii digits", () => {
    const c = unlockCode(key(0x11), "abcd");
    expect(c).toHaveLength(UNLOCK_CODE_DIGITS);
    expect(/^[0-9]{8}$/.test(c)).toBe(true);
  });

  it("changes with the challenge or the key", () => {
    expect(unlockCode(key(1), "K7QN")).not.toBe(unlockCode(key(1), "K7QM"));
    expect(unlockCode(key(1), "K7QN")).not.toBe(unlockCode(key(2), "K7QN"));
  });

  it("cross-language vector matches the Rust machine-side path", () => {
    // The Rust test `offline_unlock_code_matches_ts_guardian_side`
    // (core/crates/charter-transport/src/nip44.rs) computes this SAME code from
    // the machine side (machine_sk=[9;32], guardian_pk). If secp256k1/nip44 and
    // noble/nip44 ever diverge, one of these two asserts fails — the guard that
    // proves a parent's code actually unlocks a real device.
    const guardianSk = key(3);
    const machinePk = getPublicKey(key(9));
    expect(unlockCodeForDevice(guardianSk, machinePk, "Z9F2")).toBe("83000851");
  });

  it("guardian side and machine side derive the same code (symmetric ECDH)", () => {
    // The crux of the design: Kintrinsic computes from (guardianSk, devicePk);
    // charterd computes from (machineSk, guardianPk). Both are the same NIP-44
    // conversation key, so the codes match without either sharing a secret.
    const guardianSk = key(3);
    const machineSk = key(9);
    const machinePk = getPublicKey(machineSk);
    const guardianPk = getPublicKey(guardianSk);
    const viaGuardian = unlockCodeForDevice(guardianSk, machinePk, "Z9F2");
    const viaMachine = unlockCode(nip44.getConversationKey(machineSk, guardianPk), "Z9F2");
    expect(viaGuardian).toBe(viaMachine);
  });
});
