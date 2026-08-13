import { describe, it, expect, vi, afterEach } from "vitest";
import { webcrypto } from "node:crypto";
import { fetchReleaseApk } from "./apk";

describe("fetchReleaseApk", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("fetches the apk and computes its sha256", async () => {
    const bytes = new Uint8Array([1, 2, 3, 4]);
    vi.stubGlobal("fetch", vi.fn(async () => ({
      ok: true,
      arrayBuffer: async () => bytes.buffer,
    })));
    // jsdom's `crypto` global has no `subtle` implementation (verified: `new
    // JSDOM().window.crypto.subtle` is undefined), so apk.ts's real
    // `crypto.subtle.digest` call would throw in this test env. Stub in
    // Node's own WebCrypto implementation — a REAL SubtleCrypto, not a
    // fake — so the sha256 assertion below exercises actual digest
    // computation rather than a hardcoded/faked hash.
    vi.stubGlobal("crypto", webcrypto);

    const res = await fetchReleaseApk();
    expect(res.bytes.length).toBe(4);
    expect(res.sha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("throws when the apk is missing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404 })));
    await expect(fetchReleaseApk()).rejects.toThrow();
  });
});
