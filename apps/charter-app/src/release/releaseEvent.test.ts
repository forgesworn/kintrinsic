import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { finalizeEvent } from "nostr-tools/pure";
import { getPublicKey } from "nostr-tools";
import {
  isCanonicalBlossomUrl,
  latestRelease,
  pickInstallUrl,
  releaseFromEvent,
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
