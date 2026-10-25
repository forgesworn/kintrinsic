import { describe, expect, it } from "vitest";
import { getPublicKey, verifyEvent, type EventTemplate } from "nostr-tools";
import { localRelayAuth } from "./relayAuth";
import fixture from "../../../../core/crates/charter-testkit/vectors/nostr/relay_auth.json";

const secret = new Uint8Array(32).fill(3);
const template: EventTemplate = {
  kind: 22242, created_at: 100, content: "",
  tags: [["relay", "wss://private.example"], ["challenge", "relay-challenge"]],
};

describe("relay whitelist authentication", () => {
  it("matches the Rust-consumed NIP-42 vector", async () => {
    const key = new Uint8Array(32); key[31] = 0x11;
    const proof = await localRelayAuth(key)({ kind: fixture.event.kind, created_at: fixture.event.created_at,
      tags: fixture.event.tags, content: fixture.event.content });
    expect(proof.id).toBe(fixture.event.id);
    expect(proof.pubkey).toBe(fixture.event.pubkey);
    expect(verifyEvent(fixture.event)).toBe(true);
  });
  it("proves the stable guardian identity without changing the request", async () => {
    const event = await localRelayAuth(secret)(template);
    expect(event.pubkey).toBe(getPublicKey(secret));
    expect(event.tags).toEqual(template.tags);
    expect(verifyEvent(event)).toBe(true);
  });
  it("refuses to sign a charter change disguised as an authentication request", async () => {
    for (const invalid of [
      { ...template, kind: 31118 }, { ...template, content: "change relays" },
      { ...template, tags: [["relay", "ws://insecure.example"], ["challenge", "x"]] },
      { ...template, tags: [["relay", "wss://private.example"], ["challenge", ""]] },
    ]) {
      await expect(localRelayAuth(secret)(invalid)).rejects.toThrow("Invalid relay authentication request");
    }
  });
});
