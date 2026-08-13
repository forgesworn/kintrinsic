import { describe, expect, it } from "vitest";
import { nip19 } from "nostr-tools";
import { parseDevicePairingCode, parsePairInvite } from "./deviceCode";

const HEX = "1".repeat(64);

describe("parseDevicePairingCode", () => {
  it("decodes an npub to its hex pubkey", () => {
    expect(parseDevicePairingCode(nip19.npubEncode(HEX))).toBe(HEX);
  });

  it("accepts a raw 64-hex pubkey (lowercased)", () => {
    expect(parseDevicePairingCode("ABCDEF" + "0".repeat(58))).toBe("abcdef" + "0".repeat(58));
  });

  it("decodes an nprofile to its pubkey", () => {
    const np = nip19.nprofileEncode({ pubkey: HEX, relays: ["wss://relay.example"] });
    expect(parseDevicePairingCode(np)).toBe(HEX);
  });

  it("trims surrounding whitespace", () => {
    expect(parseDevicePairingCode(`  ${nip19.npubEncode(HEX)}\n`)).toBe(HEX);
  });

  it("accepts the on-screen grouped form (8 space-separated hex blocks)", () => {
    const hex = "abcdef01".repeat(8); // 64 hex chars
    const grouped = (hex.match(/.{1,8}/g) ?? []).join(" "); // as charterd displays it
    expect(grouped).toContain(" ");
    expect(parseDevicePairingCode(grouped)).toBe(hex);
    // Also tolerant of stray internal spaces in a hand-typed code.
    expect(parseDevicePairingCode(`  ${grouped}  \n`)).toBe(hex);
  });

  it("rejects junk / wrong length / other bech32 kinds", () => {
    expect(parseDevicePairingCode("not a key")).toBeNull();
    expect(parseDevicePairingCode("1".repeat(63))).toBeNull();
    expect(parseDevicePairingCode(nip19.noteEncode(HEX))).toBeNull();
    expect(parseDevicePairingCode("")).toBeNull();
  });
});

describe("scan-to-pair invites", () => {
  const M = "ab".repeat(32);
  const T = "cd".repeat(16);

  it("parses a scan-to-pair URI into machine + token", () => {
    expect(parsePairInvite(`charter://pair?m=${M}&t=${T}`)).toEqual({ machine: M, token: T });
  });

  it("exposes the machine key to the existing code path too", () => {
    expect(parseDevicePairingCode(`charter://pair?m=${M}&t=${T}`)).toBe(M);
  });

  it("still parses a bare device code, which carries no token", () => {
    expect(parseDevicePairingCode(M)).toBe(M);
    expect(parsePairInvite(M)).toBeNull();
  });

  it("rejects a URI whose token is not 32 hex", () => {
    expect(parsePairInvite(`charter://pair?m=${M}&t=nope`)).toBeNull();
    expect(parsePairInvite(`charter://pair?m=${M}`)).toBeNull();
  });

  it("rejects a URI whose machine key is not 64 hex", () => {
    expect(parsePairInvite(`charter://pair?m=abc&t=${T}`)).toBeNull();
  });
});
