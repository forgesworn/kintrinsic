import { afterEach, describe, expect, it } from "vitest";
import { getPublicKey } from "nostr-tools";
import {
  clearPhoneWorkingKey,
  initPhoneWorkingKey,
  mintPhoneWorkingKey,
  readPhoneWorkingKey,
  resetPhoneWorkingKeyForTests,
} from "./phoneWorkingKey";
import { loadOrCreateGuardianKey, resetGuardianKeyCacheForTests } from "./guardianKey";
import { installWorkingKey, listeningKeys } from "../store/useGuardianKeys";

afterEach(() => {
  clearPhoneWorkingKey();
  localStorage.clear();
  resetPhoneWorkingKeyForTests();
  resetGuardianKeyCacheForTests();
});

describe("a phone-held root's working key slot", () => {
  it("is empty until minted, then survives a reload, and is never the guardian key", async () => {
    expect(readPhoneWorkingKey()).toBeNull();
    const local = loadOrCreateGuardianKey();
    expect(installWorkingKey()).toEqual(local);
    expect(listeningKeys()).toHaveLength(1);

    const w = mintPhoneWorkingKey();
    expect(getPublicKey(w)).not.toBe(getPublicKey(local));
    resetPhoneWorkingKeyForTests();
    await initPhoneWorkingKey();
    expect(readPhoneWorkingKey()).toEqual(w);
    expect(installWorkingKey()).toEqual(w);
    // Ward traffic is read with both until every ward has taken W.
    expect(listeningKeys().map((k) => getPublicKey(k))).toEqual([getPublicKey(local), getPublicKey(w)]);

    clearPhoneWorkingKey();
    expect(readPhoneWorkingKey()).toBeNull();
  });
});
