import { describe, expect, it } from "vitest";
import { parseBunkerInput } from "nostr-tools/nip46";
import {
  guardianBunkerUri,
  guardianNpub,
  guardianPairingQrContent,
  mintPairToken,
} from "./guardianPairing";

const PK = "a".repeat(64);

describe("guardianBunkerUri", () => {
  it("round-trips the pubkey + relay and carries kind=charter", async () => {
    const uri = guardianBunkerUri(PK, ["wss://relay.trotters.cc"]);
    expect(uri.startsWith("bunker://")).toBe(true);
    expect(uri).toContain("kind=charter");
    const pointer = await parseBunkerInput(uri);
    expect(pointer?.pubkey).toBe(PK);
    expect(pointer?.relays).toContain("wss://relay.trotters.cc");
  });

  it("emits the relay UNENCODED — the contract grammar is a literal relay=wss://", () => {
    // The device parser (`pin_from_connect`) pins on the literal `wss://`
    // prefix. Percent-encoding here (`relay=wss%3A%2F%2F…`) produced URIs no
    // device could ever pin — the §5.1 latent bug. This is the regression pin;
    // `parseBunkerInput` above is too forgiving to catch it.
    const uri = guardianBunkerUri(PK, ["wss://relay.trotters.cc"]);
    expect(uri).toContain("relay=wss://relay.trotters.cc");
    expect(uri).not.toContain("%3A");
  });
});

describe("guardianPairingQrContent", () => {
  it("wraps the bunker URI + token in the https App Link's fragment", () => {
    const qr = guardianPairingQrContent(PK, ["wss://relay.trotters.cc"], "tok123");
    // The App Link host/path the phone's intent filter pins.
    expect(qr.startsWith("https://charter.mysignet.app/pair#bunker://")).toBe(true);
    // The fragment carries the literal bunker URI (never percent-encoded —
    // the §5.1 rule) plus the one-time token.
    expect(qr).toContain(`#bunker://${PK}?relay=wss://relay.trotters.cc&kind=charter&token=tok123`);
  });

  it("mints distinct 32-hex CSPRNG tokens", () => {
    const a = mintPairToken();
    const b = mintPairToken();
    expect(a).toMatch(/^[0-9a-f]{32}$/);
    expect(a).not.toBe(b);
  });
});

describe("guardianNpub", () => {
  it("encodes the hex pubkey as an npub", () => {
    expect(guardianNpub(PK)).toMatch(/^npub1/);
  });
});
