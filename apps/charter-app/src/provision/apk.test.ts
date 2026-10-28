import { describe, it, expect, vi, afterEach } from "vitest";
import { webcrypto, createHash } from "node:crypto";
import { fetchReleaseApk } from "./apk";

const BYTES = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
const SHA = createHash("sha256").update(BYTES).digest("hex");
const BLOSSOM = `https://nostr.download/${SHA}.apk`;
const GITHUB = "https://github.com/forgesworn/kintrinsic/releases/download/ward-v1/kintrinsic-ward-1.apk";

function feed(over: Record<string, unknown> = {}) {
  return {
    versionName: "1.0.0",
    versionCode: 1,
    url: BLOSSOM,
    // GitHub listed first, as the real feed does.
    urls: [GITHUB, BLOSSOM],
    apkSha256: SHA,
    certSha256: "0".repeat(64),
    sizeBytes: BYTES.length,
    ...over,
  };
}

type Reply = { status?: number; body?: Uint8Array | object | string; type?: string };

function stubFetch(feedBody: unknown, files: Record<string, Reply | "network-error">) {
  const calls: string[] = [];
  const fn = vi.fn(async (input: string) => {
    calls.push(input);
    if (input === "/charter-apk.json") {
      return { ok: true, status: 200, json: async () => feedBody, headers: new Headers() };
    }
    const r = files[input];
    if (r === undefined) return { ok: false, status: 404, headers: new Headers() };
    if (r === "network-error") throw new TypeError("Failed to fetch");
    const status = r.status ?? 200;
    const raw = r.body instanceof Uint8Array ? r.body : new TextEncoder().encode(String(r.body ?? ""));
    return {
      ok: status >= 200 && status < 300,
      status,
      headers: new Headers(r.type ? { "content-type": r.type } : {}),
      arrayBuffer: async () => raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength),
    };
  });
  vi.stubGlobal("fetch", fn);
  // jsdom's `crypto` has no `subtle`; use Node's real WebCrypto so the digest is genuine.
  vi.stubGlobal("crypto", webcrypto);
  return calls;
}

describe("fetchReleaseApk", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("downloads from Blossom first and returns the verified digest", async () => {
    const calls = stubFetch(feed(), { [BLOSSOM]: { body: BYTES }, [GITHUB]: { body: BYTES } });
    const res = await fetchReleaseApk();
    expect(res.sha256).toBe(SHA);
    expect(res.versionName).toBe("1.0.0");
    expect(res.bytes).toEqual(BYTES);
    expect(calls).toEqual(["/charter-apk.json", BLOSSOM]);
  });

  it("requests the feed with no-store", async () => {
    stubFetch(feed(), { [BLOSSOM]: { body: BYTES } });
    await fetchReleaseApk();
    expect((fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      "/charter-apk.json",
      { cache: "no-store" },
    ]);
  });

  it("falls back to the GitHub URL when Blossom fails", async () => {
    const calls = stubFetch(feed(), { [BLOSSOM]: "network-error", [GITHUB]: { body: BYTES } });
    const res = await fetchReleaseApk();
    expect(res.sha256).toBe(SHA);
    expect(calls).toEqual(["/charter-apk.json", BLOSSOM, GITHUB]);
  });

  it("falls back when Blossom serves the wrong bytes", async () => {
    const calls = stubFetch(feed(), {
      [BLOSSOM]: { body: new Uint8Array([9, 9, 9, 9, 9, 9, 9, 9]) },
      [GITHUB]: { body: BYTES },
    });
    expect((await fetchReleaseApk()).sha256).toBe(SHA);
    expect(calls).toEqual(["/charter-apk.json", BLOSSOM, GITHUB]);
  });

  it("compares the digest case-insensitively", async () => {
    stubFetch(feed({ apkSha256: SHA.toUpperCase() }), { [BLOSSOM]: { body: BYTES } });
    expect((await fetchReleaseApk()).sha256).toBe(SHA);
  });

  it("refuses a digest mismatch from every source", async () => {
    const wrong = new Uint8Array([9, 9, 9, 9, 9, 9, 9, 9]);
    stubFetch(feed(), { [BLOSSOM]: { body: wrong }, [GITHUB]: { body: wrong } });
    await expect(fetchReleaseApk()).rejects.toThrow(/does not match its published checksum/);
  });

  it("refuses a wrong size", async () => {
    stubFetch(feed({ sizeBytes: BYTES.length + 1 }), {
      [BLOSSOM]: { body: BYTES },
      [GITHUB]: { body: BYTES },
    });
    await expect(fetchReleaseApk()).rejects.toThrow(/wrong size/);
  });

  it("skips the size check when the feed has no sizeBytes", async () => {
    const f = feed();
    delete (f as Record<string, unknown>).sizeBytes;
    stubFetch(f, { [BLOSSOM]: { body: BYTES } });
    expect((await fetchReleaseApk()).sha256).toBe(SHA);
  });

  it("refuses an HTML page served with status 200", async () => {
    const html = "<!doctype html><html><body>Kintrinsic</body></html>";
    stubFetch(feed(), {
      [BLOSSOM]: { body: html, type: "text/html; charset=utf-8" },
      [GITHUB]: { body: html },
    });
    await expect(fetchReleaseApk()).rejects.toThrow(/web page instead of the app/);
  });

  it("refuses HTML even without a content-type, via the pinned digest", async () => {
    const html = "<!doctype html><html><body>Kintrinsic</body></html>";
    stubFetch(feed({ sizeBytes: undefined }), { [BLOSSOM]: { body: html }, [GITHUB]: { body: html } });
    await expect(fetchReleaseApk()).rejects.toThrow(/does not match its published checksum/);
  });

  it("never fetches a non-https source", async () => {
    const calls = stubFetch(
      feed({ url: "http://nostr.download/x.apk", urls: ["http://evil.example/a.apk", GITHUB] }),
      { [GITHUB]: { body: BYTES } },
    );
    expect((await fetchReleaseApk()).sha256).toBe(SHA);
    expect(calls).toEqual(["/charter-apk.json", GITHUB]);
  });

  it("refuses a feed with only non-https sources", async () => {
    stubFetch(feed({ url: "http://x.example/a.apk", urls: ["http://y.example/a.apk"] }), {});
    await expect(fetchReleaseApk()).rejects.toThrow(/no https download address/);
  });

  it("refuses a feed with no usable checksum", async () => {
    stubFetch(feed({ apkSha256: "nope" }), {});
    await expect(fetchReleaseApk()).rejects.toThrow(/no valid checksum/);
  });

  it("refuses an HTML feed (SPA fallback answering for the JSON)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => {
        throw new SyntaxError("Unexpected token <");
      },
      headers: new Headers(),
    })));
    await expect(fetchReleaseApk()).rejects.toThrow(/not in the expected form/);
  });

  it("throws when the feed is missing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404 })));
    await expect(fetchReleaseApk()).rejects.toThrow(/not found \(404\)/);
  });
});
