import { describe, expect, it } from "vitest";
import { finalizeEvent } from "nostr-tools/pure";
import { getPublicKey } from "nostr-tools";
import { fetchAllReleaseManifests } from "./fetchReleases";

const SHA = "a1ea84592cccd0e0356c1183c62d81d59c007bb8ce3b26f401c2500a122f768c";
const CERT = "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42";

function secret(low: number): Uint8Array {
  const s = new Uint8Array(32);
  s[31] = low;
  return s;
}
const SK = secret(0x33);
const PK = getPublicKey(SK);

function release(channel: string, versionCode: number, withCert = true) {
  const tags = [
    ["d", channel],
    ["version", `0.0.${versionCode}`],
    ["version_code", String(versionCode)],
    ["x", SHA],
    ["size", "1000"],
    ["url", `https://blossom.example/${SHA}`],
  ];
  if (withCert) tags.push(["cert", CERT]);
  return finalizeEvent({ kind: 30063, created_at: 1_754_900_000, tags, content: "" }, SK);
}

describe("fetchAllReleaseManifests", () => {
  it("splits one mixed query result into per-channel latest manifests", async () => {
    const events = [
      release("charter-apk", 40),
      release("charter-apk", 41),
      release("mycharter-apk", 10),
      release("charter-deb", 706, false),
      { kind: 30063, junk: true },
    ];
    const r = await fetchAllReleaseManifests(async () => events, PK);
    expect(r.ward?.versionCode).toBe(41);
    expect(r.carrier?.versionCode).toBe(10);
    expect(r.deb?.versionCode).toBe(706);
    expect(r.deb?.certSha256).toBe("");
  });

  it("resolves all-null when the query rejects", async () => {
    const r = await fetchAllReleaseManifests(async () => {
      throw new Error("relay down");
    }, PK);
    expect(r).toEqual({ ward: null, carrier: null, deb: null });
  });

  it("passes the pinned author + channels in the filter", async () => {
    let seen: { relays?: string[]; filter?: Record<string, unknown> } = {};
    await fetchAllReleaseManifests(async (relays, filter) => {
      seen = { relays, filter: filter as Record<string, unknown> };
      return [];
    }, PK);
    expect(seen.relays).toContain("wss://relay.trotters.cc");
    expect(seen.relays!.length).toBeGreaterThan(1);
    expect(seen.filter!.kinds).toEqual([30063]);
    expect(seen.filter!.authors).toEqual([PK]);
    expect(seen.filter!["#d"]).toEqual(["charter-apk", "mycharter-apk", "charter-deb"]);
  });
});
