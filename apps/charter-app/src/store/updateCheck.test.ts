import { describe, expect, it } from "vitest";
import { parseManifest, updateAvailable, fetchManifest , parseDebManifest } from "./updateCheck";
import type { UpdateManifest } from "../wire/types";

const GOOD: UpdateManifest = {
  versionName: "0.21.0",
  versionCode: 21,
  apkSha256: "a".repeat(64),
  certSha256: "b".repeat(64),
  sizeBytes: 52428800,
  builtAt: "2026-07-21T22:00:00Z",
};

describe("parseManifest", () => {
  it("accepts a well-formed manifest", () => {
    expect(parseManifest({ ...GOOD })).toEqual(GOOD);
  });

  it("accepts and validates the optional artifact path", () => {
    expect(parseManifest({ ...GOOD, path: "/charter-0.2.0.apk.txt" })).toEqual({
      ...GOOD,
      path: "/charter-0.2.0.apk.txt",
    });
    expect(parseManifest({ ...GOOD, path: "not-rooted" })).toBeNull();
    expect(parseManifest({ ...GOOD, path: 7 })).toBeNull();
  });

  it("rejects junk fail-quiet", () => {
    expect(parseManifest(null)).toBeNull();
    expect(parseManifest("nope")).toBeNull();
    expect(parseManifest({ ...GOOD, versionCode: 0 })).toBeNull();
    expect(parseManifest({ ...GOOD, versionCode: 1.5 })).toBeNull();
    expect(parseManifest({ ...GOOD, apkSha256: "xyz" })).toBeNull();
    expect(parseManifest({ ...GOOD, apkSha256: "A".repeat(64) })).toBeNull();
    expect(parseManifest({ ...GOOD, certSha256: undefined })).toBeNull();
    expect(parseManifest({ ...GOOD, versionName: "" })).toBeNull();
  });
});

describe("updateAvailable", () => {
  it("offers only when the device reports a version strictly behind", () => {
    expect(updateAvailable(GOOD, 20)).toBe(true);
    expect(updateAvailable(GOOD, 21)).toBe(false);
    expect(updateAvailable(GOOD, 22)).toBe(false);
  });

  it("offers nothing without a manifest or a reported version", () => {
    expect(updateAvailable(null, 20)).toBe(false);
    expect(updateAvailable(GOOD, undefined)).toBe(false);
  });
});

describe("fetchManifest", () => {
  it("parses a good response", async () => {
    const fetcher = (async () => ({
      ok: true,
      json: async () => ({ ...GOOD }),
    })) as unknown as typeof fetch;
    expect(await fetchManifest(fetcher)).toEqual(GOOD);
  });

  it("is fail-quiet on HTTP error and on throw", async () => {
    const bad = (async () => ({ ok: false, json: async () => ({}) })) as unknown as typeof fetch;
    expect(await fetchManifest(bad)).toBeNull();
    const boom = (async () => {
      throw new Error("offline");
    }) as unknown as typeof fetch;
    expect(await fetchManifest(boom)).toBeNull();
  });
});

describe("parseDebManifest", () => {
  const good = {
    versionName: "0.3.6",
    versionCode: 306,
    path: "/charter-latest.deb",
    sha256: "a".repeat(64),
    sizeBytes: 6568784,
    builtAt: "2026-07-26T00:00:00Z",
  };

  it("accepts what publish-deb.sh writes", () => {
    expect(parseDebManifest(good)).toEqual({
      versionName: "0.3.6",
      versionCode: 306,
      path: "/charter-latest.deb",
      sizeBytes: 6568784,
      builtAt: "2026-07-26T00:00:00Z",
    });
  });

  it("rejects anything malformed rather than half-trusting it", () => {
    expect(parseDebManifest(null)).toBeNull();
    expect(parseDebManifest({ ...good, versionName: "" })).toBeNull();
    expect(parseDebManifest({ ...good, versionCode: 0 })).toBeNull();
    expect(parseDebManifest({ ...good, versionCode: 1.5 })).toBeNull();
    expect(parseDebManifest({ ...good, path: "charter.deb" })).toBeNull();
    expect(parseDebManifest({ ...good, sizeBytes: 0 })).toBeNull();
  });

  /**
   * The ONE comparison serves both platforms, so the deb's derived code must
   * order the same way charterd's does (0.3.6 -> 306).
   */
  it("feeds the same update comparison as the APK manifest", () => {
    const m = parseDebManifest(good)!;
    expect(updateAvailable(m as never, 305)).toBe(true);
    expect(updateAvailable(m as never, 306)).toBe(false);
    expect(updateAvailable(m as never, 307)).toBe(false);
    // A laptop that reports nothing is never told it's behind.
    expect(updateAvailable(m as never, undefined)).toBe(false);
  });
});
