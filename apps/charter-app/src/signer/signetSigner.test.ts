import { describe, expect, it } from "vitest";
import { SimplePool } from "nostr-tools";
import { createSignetSigner } from "./signetSigner";

// Construction smoke test only — the live bunker/relay IO is exercised against a
// running Signet, not here. We assert the factory wires a disconnected RealSigner
// without touching the network (SimplePool connects lazily on publish/sub).
describe("createSignetSigner", () => {
  it("constructs a disconnected RealSigner with no network IO", () => {
    const signer = createSignetSigner({
      getBunkerUri: async () => "bunker://deadbeef?relay=wss://relay.example",
      resolveChild: () => undefined,
      pool: new SimplePool(),
    });
    expect(signer.status()).toMatchObject({ connected: false, kind: "none" });
  });
});
