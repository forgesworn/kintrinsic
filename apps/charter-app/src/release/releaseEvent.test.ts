import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { finalizeEvent } from "nostr-tools/pure";
import { getPublicKey } from "nostr-tools";
import {
  allFollowRedirects,
  isCanonicalBlossomUrl,
  isGithubReleaseUrl,
  latestRelease,
  pickInstallUrl,
  releaseFromEvent,
  shellInstallUrl,
  SHELL_FOLLOWS_REDIRECTS_FROM,
  WARD_FOLLOWS_REDIRECTS_FROM,
} from "./releaseEvent";

const SHA = "a1ea84592cccd0e0356c1183c62d81d59c007bb8ce3b26f401c2500a122f768c";
const CERT = "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42";
const TS = 1_754_900_000;

function secret(low: number): Uint8Array {
  const s = new Uint8Array(32);
  s[31] = low;
  return s;
}
const SK = secret(0x33);
const PK = getPublicKey(SK);

function tags(channel: string): string[][] {
  return [
    ["d", channel],
    ["version", "0.6.9"],
    ["version_code", "40"],
    ["x", SHA],
    ["size", "27693181"],
    ["cert", CERT],
    ["url", `https://blossom.example/${SHA}`],
    ["url", `https://mirror.example/${SHA}`],
  ];
}

function signed(t: string[][], overrides: object = {}) {
  return finalizeEvent(
    { kind: 30063, created_at: TS, tags: t, content: "notes", ...overrides },
    SK,
  );
}

function without(t: string[][], name: string): string[][] {
  return t.filter((tag) => tag[0] !== name);
}

describe("releaseFromEvent", () => {
  it("verifies and maps every field", () => {
    const r = releaseFromEvent(signed(tags("charter-apk")), "charter-apk", PK);
    expect(r).not.toBeNull();
    expect(r!.versionName).toBe("0.6.9");
    expect(r!.versionCode).toBe(40);
    expect(r!.apkSha256).toBe(SHA);
    expect(r!.certSha256).toBe(CERT);
    expect(r!.sizeBytes).toBe(27_693_181);
    expect(r!.builtAt).toBe(new Date(TS * 1000).toISOString());
    expect(r!.urls).toEqual([
      `https://blossom.example/${SHA}`,
      `https://mirror.example/${SHA}`,
    ]);
  });

  it("rejects an event signed by a different key than the pin", () => {
    const ev = signed(tags("charter-apk"));
    const otherPin = getPublicKey(secret(0x44));
    expect(releaseFromEvent(ev, "charter-apk", otherPin)).toBeNull();
  });

  it("rejects the wrong channel and the wrong kind", () => {
    const ev = signed(tags("charter-apk"));
    expect(releaseFromEvent(ev, "mycharter-apk", PK)).toBeNull();
    const wrongKind = signed(tags("charter-apk"), { kind: 31116 });
    expect(releaseFromEvent(wrongKind, "charter-apk", PK)).toBeNull();
  });

  it("rejects a tampered event (id/sig no longer match)", () => {
    const ev = signed(tags("charter-apk"));
    const tampered = {
      ...ev,
      tags: ev.tags.map((t) => (t[0] === "version_code" ? ["version_code", "9999"] : t)),
    };
    expect(releaseFromEvent(tampered, "charter-apk", PK)).toBeNull();
  });

  it("rejects missing required fields", () => {
    for (const name of ["version", "version_code", "x", "size", "url"]) {
      const ev = signed(without(tags("charter-apk"), name));
      expect(releaseFromEvent(ev, "charter-apk", PK), `missing ${name}`).toBeNull();
    }
  });

  it("rejects zero version_code, bad sha, and non-https mirrors", () => {
    const zero = tags("charter-apk").map((t) =>
      t[0] === "version_code" ? ["version_code", "0"] : t,
    );
    expect(releaseFromEvent(signed(zero), "charter-apk", PK)).toBeNull();

    const upper = tags("charter-apk").map((t) => (t[0] === "x" ? ["x", SHA.toUpperCase()] : t));
    expect(releaseFromEvent(signed(upper), "charter-apk", PK)).toBeNull();

    const http = [...tags("charter-apk"), ["url", `http://evil.example/${SHA}`]];
    expect(releaseFromEvent(signed(http), "charter-apk", PK)).toBeNull();
  });

  it("deb channel needs no cert; APK channels require one", () => {
    const deb = signed(without(tags("charter-deb"), "cert"));
    const r = releaseFromEvent(deb, "charter-deb", PK);
    expect(r).not.toBeNull();
    expect(r!.certSha256).toBe("");

    const apkNoCert = signed(without(tags("charter-apk"), "cert"));
    expect(releaseFromEvent(apkNoCert, "charter-apk", PK)).toBeNull();
  });

  it("rejects junk shapes without throwing", () => {
    for (const junk of [null, undefined, 42, "ev", {}, { kind: 30063 }]) {
      expect(releaseFromEvent(junk, "charter-apk", PK)).toBeNull();
    }
  });
});

describe("latestRelease", () => {
  it("picks the highest versionCode among verified events, skipping junk", () => {
    const v40 = signed(tags("charter-apk"));
    const v41tags = tags("charter-apk").map((t) =>
      t[0] === "version_code" ? ["version_code", "41"] : t,
    );
    const v41 = signed(v41tags);
    const junk = { kind: 30063 };
    const best = latestRelease([v40, junk, v41], "charter-apk", PK);
    expect(best?.versionCode).toBe(41);
  });

  it("returns null when nothing verifies", () => {
    expect(latestRelease([{ kind: 1 }, null], "charter-apk", PK)).toBeNull();
  });
});

describe("golden vector parity with the Rust verifier", () => {
  it("parses the shared fixture to the fixture's expected fields", () => {
    // vitest cwd is apps/charter-app; import.meta.url is not file:// under jsdom.
    const raw = readFileSync(
      join(process.cwd(), "../../core/crates/charter-testkit/vectors/nostr/software_release.json"),
      "utf8",
    );
    const v = JSON.parse(raw);
    const r = releaseFromEvent(v.event, v.expected.channel, v.release_pubkey);
    expect(r).not.toBeNull();
    expect(r!.versionName).toBe(v.expected.versionName);
    expect(r!.versionCode).toBe(v.expected.versionCode);
    expect(r!.apkSha256).toBe(v.expected.sha256);
    expect(r!.certSha256).toBe(v.expected.certSha256);
    expect(r!.sizeBytes).toBe(v.expected.sizeBytes);
    expect(r!.urls).toEqual(v.expected.urls);
  });

  it("the fixture leads with the GitHub Release; only a redirect-following client is handed it", () => {
    const raw = readFileSync(
      join(process.cwd(), "../../core/crates/charter-testkit/vectors/nostr/software_release.json"),
      "utf8",
    );
    const v = JSON.parse(raw);
    expect(isGithubReleaseUrl(v.expected.urls[0])).toBe(true);
    expect(pickInstallUrl(v.expected.urls, v.expected.sha256, true)).toBe(v.expected.urls[0]);
    expect(pickInstallUrl(v.expected.urls, v.expected.sha256)).toBe(v.expected.urls[1]);
  });
});

describe("isCanonicalBlossomUrl", () => {
  it("accepts only the blob's root address for this sha", () => {
    expect(isCanonicalBlossomUrl(`https://nostr.download/${SHA}`, SHA)).toBe(true);
    expect(isCanonicalBlossomUrl(`https://nostr.download/${SHA}.apk`, SHA)).toBe(true);
    expect(isCanonicalBlossomUrl(`https://media.primal.net/uploads2/a/1e/a8/${SHA}`, SHA)).toBe(
      false,
    );
    expect(isCanonicalBlossomUrl(`http://nostr.download/${SHA}`, SHA)).toBe(false);
    expect(isCanonicalBlossomUrl(`https://nostr.download/${"b".repeat(64)}`, SHA)).toBe(false);
    expect(isCanonicalBlossomUrl(`https://nostr.download/${SHA}?x`, SHA)).toBe(false);
    expect(isCanonicalBlossomUrl("not a url", SHA)).toBe(false);
  });
});

describe("pickInstallUrl", () => {
  it("prefers a canonical Blossom address over a CDN redirect target (the 0.6.9 event)", () => {
    const urls = [
      `https://media.primal.net/uploads2/a/1e/a8/${SHA}`,
      `https://nostr.download/${SHA}`,
    ];
    expect(pickInstallUrl(urls, SHA)).toBe(`https://nostr.download/${SHA}`);
  });

  it("prefers the extension-bearing canonical form", () => {
    const urls = [`https://nostr.download/${SHA}`, `https://blossom.primal.net/${SHA}.apk`];
    expect(pickInstallUrl(urls, SHA)).toBe(`https://blossom.primal.net/${SHA}.apk`);
  });

  it("keeps event order within a class", () => {
    const urls = [`https://a.example/${SHA}.apk`, `https://b.example/${SHA}.apk`];
    expect(pickInstallUrl(urls, SHA)).toBe(`https://a.example/${SHA}.apk`);
  });

  it("falls back to the first url when nothing is canonical, and null on empty", () => {
    expect(pickInstallUrl([`https://x.example/dl/${SHA}`, `https://y.example/z`], SHA)).toBe(
      `https://x.example/dl/${SHA}`,
    );
    expect(pickInstallUrl([], SHA)).toBeNull();
  });
});

const GH = "https://github.com/forgesworn/kintrinsic/releases/download";

describe("isGithubReleaseUrl", () => {
  it("accepts only our repo's per-artifact release downloads", () => {
    expect(isGithubReleaseUrl(`${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`)).toBe(true);
    expect(isGithubReleaseUrl(`${GH}/guardian-v0.1.15/kintrinsic-0.1.15.apk`)).toBe(true);
    expect(isGithubReleaseUrl(`${GH}/linux-v0.7.10/kintrinsic_0.7.10_amd64.deb`)).toBe(true);
    expect(
      isGithubReleaseUrl("https://github.com/evil/kintrinsic/releases/download/ward-v1/x.apk"),
    ).toBe(false);
    expect(isGithubReleaseUrl(`${GH}/v0.6.13/x.apk`)).toBe(false);
    expect(isGithubReleaseUrl(`${GH}/ward-v0.6.13/x.apk?raw=1`)).toBe(false);
    expect(isGithubReleaseUrl(`${GH.replace("https", "http")}/ward-v1/x.apk`)).toBe(false);
    expect(
      isGithubReleaseUrl("https://release-assets.githubusercontent.com/github-production-release-asset/1/x"),
    ).toBe(false);
    expect(isGithubReleaseUrl("not a url")).toBe(false);
  });
});

describe("pickInstallUrl with GitHub Releases", () => {
  const gh = `${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`;
  const blossom = `https://nostr.download/${SHA}.apk`;
  const urls = [gh, blossom, `https://blossom.primal.net/${SHA}`];
  const forWard = (...codes: (number | undefined)[]) =>
    pickInstallUrl(urls, SHA, allFollowRedirects(codes, WARD_FOLLOWS_REDIRECTS_FROM));

  it("an old ward (0.6.12 and earlier) is named the direct Blossom address", () => {
    expect(forWard(43)).toBe(blossom);
    expect(forWard(40)).toBe(blossom);
  });

  it("a new ward (0.6.13+) is named the GitHub Release", () => {
    expect(forWard(44)).toBe(gh);
    expect(forWard(50)).toBe(gh);
  });

  it("an unknown or mixed ward is named Blossom", () => {
    expect(forWard(undefined)).toBe(blossom);
    expect(forWard()).toBe(blossom); // no phones reported at all
    expect(forWard(44, 43)).toBe(blossom); // one old phone holds the line
    expect(forWard(44, undefined)).toBe(blossom);
  });

  it("an old guardian shell (0.1.14 and earlier) gets Blossom, a new one GitHub", () => {
    expect(shellInstallUrl(urls, SHA, 15)).toBe(blossom);
    expect(shellInstallUrl(urls, SHA, 12)).toBe(blossom);
    expect(shellInstallUrl(urls, SHA, undefined)).toBe(blossom);
    expect(shellInstallUrl(urls, SHA, SHELL_FOLLOWS_REDIRECTS_FROM)).toBe(gh);
  });

  it("GitHub wherever it sits, for a client that follows redirects", () => {
    expect(pickInstallUrl([blossom, `https://blossom.primal.net/${SHA}`, gh], SHA, true)).toBe(gh);
  });

  it("never prefers a foreign GitHub repo over a canonical Blossom address", () => {
    const foreign = "https://github.com/evil/fork/releases/download/ward-v0.6.13/x.apk";
    expect(pickInstallUrl([foreign, blossom], SHA, true)).toBe(blossom);
  });
});
