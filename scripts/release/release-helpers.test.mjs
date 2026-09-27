// node --test scripts/release/
import test from "node:test";
import assert from "node:assert/strict";
import {
  buildBlossomAuth,
  buildReleaseEvent,
  githubRelease,
  isCanonicalBlossomUrl,
  isGithubReleaseUrl,
  orderReleaseUrls,
  RELEASE_RELAYS,
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

// ---- GitHub Releases as the primary host ----------------------------------

const GH = "https://github.com/forgesworn/kintrinsic/releases/download";

test("githubRelease names one tag per artifact", () => {
  assert.deepEqual(githubRelease("charter-apk", "0.6.13"), {
    repo: "forgesworn/kintrinsic",
    tag: "ward-v0.6.13",
    asset: "kintrinsic-ward-0.6.13.apk",
    title: "Ward 0.6.13",
    url: `${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`,
  });
  assert.equal(
    githubRelease("mycharter-apk", "0.1.15").url,
    `${GH}/guardian-v0.1.15/kintrinsic-0.1.15.apk`,
  );
  assert.equal(
    githubRelease("charter-deb", "0.7.10").url,
    `${GH}/linux-v0.7.10/kintrinsic_0.7.10_amd64.deb`,
  );
  assert.throws(() => githubRelease("nope", "1.0.0"), /bad channel/);
  assert.throws(() => githubRelease("charter-apk", "../x"), /bad version/);
  assert.throws(() => githubRelease("charter-apk", ""), /bad version/);
});

test("isGithubReleaseUrl accepts only our repo's per-artifact release downloads", () => {
  assert.equal(isGithubReleaseUrl(`${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`), true);
  assert.equal(isGithubReleaseUrl(`${GH}/linux-v0.7.10/kintrinsic_0.7.10_amd64.deb`), true);
  // Another repo, another tag scheme, a query, plain http, a CDN target: no.
  assert.equal(
    isGithubReleaseUrl("https://github.com/evil/kintrinsic/releases/download/ward-v1/x.apk"),
    false,
  );
  assert.equal(isGithubReleaseUrl(`${GH}/v0.6.13/kintrinsic-ward-0.6.13.apk`), false);
  assert.equal(isGithubReleaseUrl(`${GH}/ward-v0.6.13/x.apk?raw=1`), false);
  assert.equal(isGithubReleaseUrl(`http://github.com/forgesworn/kintrinsic/releases/download/ward-v1/x.apk`), false);
  assert.equal(
    isGithubReleaseUrl("https://release-assets.githubusercontent.com/github-production-release-asset/1/x"),
    false,
  );
  assert.equal(isGithubReleaseUrl(`${GH}/ward-v0.6.13/a/b.apk`), false);
  assert.equal(isGithubReleaseUrl(42), false);
});

test("orderReleaseUrls puts GitHub first and keeps Blossom order", () => {
  const gh = `${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`;
  const a = `https://nostr.download/${SHA}.apk`;
  const b = `https://blossom.primal.net/${SHA}`;
  assert.deepEqual(orderReleaseUrls([a, b, gh, a]), [gh, a, b]);
});

test("buildReleaseEvent carries the GitHub download first, then Blossom", () => {
  const gh = `${GH}/ward-v0.6.9/kintrinsic-ward-0.6.9.apk`;
  const ev = buildReleaseEvent({ ...base, urls: [gh, `https://nostr.download/${SHA}.apk`] });
  assert.deepEqual(
    ev.tags.filter((t) => t[0] === "url"),
    [
      ["url", gh],
      ["url", `https://nostr.download/${SHA}.apk`],
    ],
  );
  assert.throws(
    () =>
      buildReleaseEvent({
        ...base,
        urls: ["https://github.com/evil/fork/releases/download/ward-v0.6.9/x.apk"],
      }),
    /GitHub Release download or/,
  );
});

test("release relays lead with trotters and keep public relays", () => {
  assert.equal(RELEASE_RELAYS[0], "wss://relay.trotters.cc");
  assert.ok(RELEASE_RELAYS.length >= 3);
});
