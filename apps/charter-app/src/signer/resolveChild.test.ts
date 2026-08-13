import { describe, expect, it } from "vitest";
import type { Child, Device } from "../domain/types";
import { resolveChildTarget } from "./resolveChild";

const RELAYS = ["wss://relay.example"];
const dev = (over: Partial<Device>): Device => ({
  id: "d",
  label: "laptop",
  platform: "linux",
  pairing: "paired",
  devicePubkey: "f".repeat(64),
  ...over,
});
const child = (over: Partial<Child>): Child => ({
  id: "child_sam",
  name: "Sam",
  color: "#abc",
  dependantPubkey: "a".repeat(64),
  devices: [],
  policies: [],
  ...over,
});

describe("resolveChildTarget", () => {
  it("collects every PAIRED device's pubkey + the child's subject + relays", () => {
    const c = child({
      devices: [
        dev({ id: "d1", devicePubkey: "1".repeat(64) }),
        dev({ id: "d2", devicePubkey: "2".repeat(64) }),
      ],
    });
    const t = resolveChildTarget([c], "child_sam", RELAYS);
    expect(t).toEqual({
      subject: "a".repeat(64),
      // The display name rides along so errors say "Sam", not `child_sam`.
      name: "Sam",
      devices: [
        { id: "d1", pubkey: "1".repeat(64) },
        { id: "d2", pubkey: "2".repeat(64) },
      ],
      relays: RELAYS,
    });
  });

  it("excludes unpaired devices and devices missing a pubkey", () => {
    const c = child({
      devices: [
        dev({ id: "ok", devicePubkey: "1".repeat(64), pairing: "paired" }),
        dev({ id: "pairing", pairing: "pairing" }),
        dev({ id: "unpaired", pairing: "unpaired" }),
        dev({ id: "nokey", devicePubkey: null }),
      ],
    });
    const t = resolveChildTarget([c], "child_sam", RELAYS);
    expect(t?.devices).toEqual([{ id: "ok", pubkey: "1".repeat(64) }]);
  });

  it("SKIPS non-hex (mock/demo) device keys — never feeds them to gift-wrap", () => {
    // A demo mockpub_ laptop beside a real phone must not crash the sign: the
    // real key survives, the fake is dropped (the full-UI-e2e regression).
    const c = child({
      devices: [
        dev({ id: "mock", devicePubkey: "mockpub_abc123" }),
        dev({ id: "real", devicePubkey: "1".repeat(64) }),
        dev({ id: "upper", devicePubkey: "A".repeat(64) }), // uppercase = not our lowercase-hex
        dev({ id: "short", devicePubkey: "1".repeat(63) }),
      ],
    });
    const t = resolveChildTarget([c], "child_sam", RELAYS);
    expect(t?.devices).toEqual([{ id: "real", pubkey: "1".repeat(64) }]);
  });

  it("returns undefined for an unknown child", () => {
    expect(resolveChildTarget([child({})], "ghost", RELAYS)).toBeUndefined();
  });

  it("passes through a null subject (single-child / not yet bound)", () => {
    const t = resolveChildTarget([child({ dependantPubkey: null, devices: [dev({})] })], "child_sam", RELAYS);
    expect(t?.subject).toBeNull();
  });
});
