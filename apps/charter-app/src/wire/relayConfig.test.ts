import { describe, expect, it } from "vitest";
import { finalizeEvent, verifyEvent } from "nostr-tools";
import { relayConfigTemplate, validateRelays, CHARTER_DEVICE_RELAYS } from "./relayConfig";
import { DEFAULT_RELAYS } from "../signer/config";
import vector from "../../../../core/crates/charter-testkit/vectors/nostr/relay_config.json";

describe("offline relay configuration", () => {
  it("matches the Rust-consumed nostr-tools vector", () => {
    expect(vector.relays).toEqual(DEFAULT_RELAYS);
    const event = relayConfigTemplate(vector.machine, vector.relays, vector.event.created_at);
    expect(event).toEqual({
      kind: vector.event.kind, created_at: vector.event.created_at,
      tags: vector.event.tags, content: vector.event.content,
    });
    expect(verifyEvent(vector.event)).toBe(true);
    expect(CHARTER_DEVICE_RELAYS).toBe(31118);
  });

  it("normalises endpoints before checking redundancy", () => {
    expect(validateRelays([" wss://ONE.example:443/ ", "wss://two.example/path"]))
      .toEqual(["wss://one.example", "wss://two.example/path"]);
    expect(() => validateRelays(["wss://ONE.example", "wss://one.example:443/"])).toThrow("two different");
    expect(validateRelays(["wss://one.example/?", "wss://two.example/"]))
      .toEqual(["wss://one.example/?", "wss://two.example"]);
    expect(() => validateRelays(["wss://one.example/#", "wss://two.example"])).toThrow();
  });

  it("rejects unsafe, missing or excessive endpoints", () => {
    for (const relays of [[], ["wss://one.example"], ["ws://one.example", "wss://two.example"],
      ["wss://user@one.example", "wss://two.example"], ["wss://one.example#fragment", "wss://two.example"],
      Array<string>(11).fill("wss://one.example")]) {
      expect(() => validateRelays(relays)).toThrow();
    }
  });

  it("signs a device-bound code that detects tampering", () => {
    const sk = new Uint8Array(32).fill(1);
    const signed = finalizeEvent(relayConfigTemplate("a".repeat(64), vector.relays, 100), sk);
    expect(verifyEvent(signed)).toBe(true);
    const tampered = JSON.parse(JSON.stringify(signed)) as typeof signed;
    tampered.content = "{}";
    expect(verifyEvent(tampered)).toBe(false);
    expect(() => relayConfigTemplate("invalid", vector.relays, 100)).toThrow();
  });
});
