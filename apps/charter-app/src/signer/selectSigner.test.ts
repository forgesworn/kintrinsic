import { afterEach, describe, expect, it } from "vitest";
import { clearGuardianKey } from "./guardianKey";
import { selectSigner } from "./selectSigner";

const deps = { resolveChild: () => undefined, childPolicies: () => [], confirmGate: () => true };

afterEach(() => clearGuardianKey());

describe("selectSigner", () => {
  it("selects a working local signer for kind 'local' (connects offline)", async () => {
    const s = selectSigner({ connected: false, kind: "local", autoSign: false }, deps);
    const st = await s.connect("local");
    expect(st).toMatchObject({ connected: true, kind: "local", label: "This phone" });
  });

  it("selects the Signet signer when a bunkerUri is present", () => {
    const s = selectSigner(
      { connected: false, kind: "signet", autoSign: false, bunkerUri: "bunker://deadbeef?relay=wss://r" },
      deps,
    );
    expect(s.status()).toMatchObject({ connected: false, kind: "none" }); // RealSigner pre-connect
  });

  it("falls back to the mock signer otherwise", () => {
    const s = selectSigner({ connected: false, kind: "none", autoSign: true }, deps);
    expect(s.status()).toMatchObject({ autoSign: true });
  });

  it("carries persisted autoSign into the local signer", async () => {
    const s = selectSigner({ connected: false, kind: "local", autoSign: true }, deps);
    await s.connect("local");
    expect(s.status().autoSign).toBe(true);
  });
});
