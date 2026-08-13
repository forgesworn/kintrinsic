import { describe, expect, it } from "vitest";
import { generateSecretKey, getPublicKey } from "nostr-tools";
import {
  backupFingerprint,
  decryptGuardianKey,
  encryptGuardianKey,
  KeyBackupError,
  encryptGuardianBackup,
  decryptGuardianBackup,
} from "./keyBackup";

const PASS = "correct horse battery";

describe("guardian key backup", () => {
  it("round-trips the exact secret under the right passphrase", async () => {
    const sk = generateSecretKey();
    const blob = await encryptGuardianKey(sk, PASS);
    const back = await decryptGuardianKey(blob, PASS);
    expect(Array.from(back)).toEqual(Array.from(sk));
    // The restored key is the SAME guardian identity — the whole point.
    expect(getPublicKey(back)).toBe(getPublicKey(sk));
  });

  it("emits a self-describing, ASCII, salted blob (fresh each time)", async () => {
    const sk = generateSecretKey();
    const a = await encryptGuardianKey(sk, PASS);
    const b = await encryptGuardianKey(sk, PASS);
    expect(a.startsWith("CHARTER-KEYBAK.1.")).toBe(true);
    expect(a.split(".")).toHaveLength(5);
    expect(/^[\x20-\x7e]+$/.test(a)).toBe(true); // printable ASCII
    expect(a).not.toBe(b); // random salt + iv ⇒ different ciphertext
  });

  it("rejects the wrong passphrase (GCM auth fails)", async () => {
    const blob = await encryptGuardianKey(generateSecretKey(), PASS);
    await expect(decryptGuardianKey(blob, "wrong pass phrase")).rejects.toBeInstanceOf(
      KeyBackupError,
    );
  });

  it("rejects a non-backup / corrupted / wrong-version blob", async () => {
    await expect(decryptGuardianKey("hello", PASS)).rejects.toBeInstanceOf(KeyBackupError);
    const good = await encryptGuardianKey(generateSecretKey(), PASS);
    const tampered = good.slice(0, -4) + "AAAA";
    await expect(decryptGuardianKey(tampered, PASS)).rejects.toBeInstanceOf(KeyBackupError);
    await expect(
      decryptGuardianKey(good.replace("KEYBAK.1.", "KEYBAK.9."), PASS),
    ).rejects.toThrow(/version/);
  });

  it("refuses a too-short passphrase and a wrong-size secret", async () => {
    await expect(encryptGuardianKey(generateSecretKey(), "short")).rejects.toBeInstanceOf(
      KeyBackupError,
    );
    await expect(encryptGuardianKey(new Uint8Array(31), PASS)).rejects.toBeInstanceOf(
      KeyBackupError,
    );
  });

  it("previews the restored fingerprint without needing to install it", async () => {
    const sk = generateSecretKey();
    const blob = await encryptGuardianKey(sk, PASS);
    expect(await backupFingerprint(blob, PASS)).toBe(getPublicKey(sk));
  });
});

describe("v2 household backup", () => {
  const secret = new Uint8Array(32).fill(7);
  const state = JSON.stringify({ children: [{ id: "c1", name: "Robin" }], requests: [] });

  it("roundtrips key + state", async () => {
    const blob = await encryptGuardianBackup(secret, state, "hunter22222");
    expect(blob.startsWith("CHARTER-KEYBAK.2.")).toBe(true);
    const back = await decryptGuardianBackup(blob, "hunter22222");
    expect(Array.from(back.secret)).toEqual(Array.from(secret));
    expect(back.stateJson).toBe(state);
  });

  it("key-only v2 has no stateJson", async () => {
    const back = await decryptGuardianBackup(
      await encryptGuardianBackup(secret, null, "hunter22222"),
      "hunter22222",
    );
    expect(back.stateJson).toBeUndefined();
  });

  it("still restores a v1 (key-only) blob", async () => {
    const v1 = await encryptGuardianKey(secret, "hunter22222");
    expect(v1.startsWith("CHARTER-KEYBAK.1.")).toBe(true);
    const back = await decryptGuardianBackup(v1, "hunter22222");
    expect(Array.from(back.secret)).toEqual(Array.from(secret));
    expect(back.stateJson).toBeUndefined();
  });

  it("wrong passphrase fails closed", async () => {
    const blob = await encryptGuardianBackup(secret, state, "hunter22222");
    await expect(decryptGuardianBackup(blob, "wrong-pass-9")).rejects.toThrow();
  });
});
