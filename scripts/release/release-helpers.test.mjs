// node --test scripts/release/
import test from "node:test";
import assert from "node:assert/strict";
import {
  buildBlossomAuth,
  buildReleaseEvent,
  isCanonicalBlossomUrl,
} from "./release-helpers.mjs";

const SHA = "a1ea84592cccd0e0356c1183c62d81d59c007bb8ce3b26f401c2500a122f768c";
const CERT = "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42";
const TS = 1_754_900_000;

const base = {
  channel: "charter-apk",
  versionName: "0.6.9",
  versionCode: 40,
  sha256: SHA,
  sizeBytes: 27_693_181,
  urls: [`https://blossom.example/${SHA}`, `https://mirror.example/${SHA}`],
  certSha256: CERT,
  notes: "notes",
  createdAt: TS,
};

test("buildReleaseEvent emits the exact tag shape", () => {
  const ev = buildReleaseEvent(base);
  assert.equal(ev.kind, 30063);
  assert.equal(ev.created_at, TS);
  assert.equal(ev.content, "notes");
  assert.deepEqual(ev.tags, [
    ["d", "charter-apk"],
    ["version", "0.6.9"],
    ["version_code", "40"],
    ["x", SHA],
    ["size", "27693181"],
    ["cert", CERT],
    ["url", `https://blossom.example/${SHA}`],
    ["url", `https://mirror.example/${SHA}`],
  ]);
});

test("charter-deb needs no cert and emits none", () => {
  const ev = buildReleaseEvent({ ...base, channel: "charter-deb", certSha256: undefined });
  assert.ok(!ev.tags.some((t) => t[0] === "cert"));
});

test("invalid fields throw", () => {
  assert.throws(() => buildReleaseEvent({ ...base, channel: "nope" }), /bad channel/);
  assert.throws(() => buildReleaseEvent({ ...base, versionCode: 0 }), /versionCode/);
  assert.throws(() => buildReleaseEvent({ ...base, sha256: SHA.toUpperCase() }), /sha256/);
  assert.throws(() => buildReleaseEvent({ ...base, urls: [] }), /url/);
  assert.throws(
    () => buildReleaseEvent({ ...base, urls: [`http://x.example/${SHA}`] }),
    /https/,
  );
  assert.throws(() => buildReleaseEvent({ ...base, certSha256: undefined }), /cert/);
  assert.throws(() => buildReleaseEvent({ ...base, sizeBytes: -1 }), /sizeBytes/);
});

test("buildBlossomAuth emits BUD-02 shape with a 10-minute expiry", () => {
  const ev = buildBlossomAuth({ sha256: SHA, createdAt: TS });
  assert.equal(ev.kind, 24242);
  assert.deepEqual(ev.tags, [
    ["t", "upload"],
    ["x", SHA],
    ["expiration", String(TS + 600)],
  ]);
  assert.throws(() => buildBlossomAuth({ sha256: "zz", createdAt: TS }), /sha256/);
});

test("isCanonicalBlossomUrl accepts only the blob's root address", () => {
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}`, SHA), true);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}.apk`, SHA), true);
  assert.equal(isCanonicalBlossomUrl(`https://blossom.primal.net/${SHA}.deb`, SHA), true);
  // The 2026-08-27 outage: primal's CDN redirect target is not an address.
  assert.equal(
    isCanonicalBlossomUrl(`https://media.primal.net/uploads2/a/1e/a8/${SHA}`, SHA),
    false,
  );
  assert.equal(isCanonicalBlossomUrl(`http://nostr.download/${SHA}`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${"b".repeat(64)}`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}?x=1`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}.tar.gz`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(42, SHA), false);
});

test("buildReleaseEvent refuses a non-canonical mirror url", () => {
  assert.throws(
    () =>
      buildReleaseEvent({
        ...base,
        urls: [`https://media.primal.net/uploads2/a/1e/a8/${SHA}`],
      }),
    /canonical root address/,
  );
});
