// Fetches the ward (child's phone) Kintrinsic APK for WebUSB provisioning and
// refuses to hand it on unless it is the exact release the feed names.
//
// The feed is `/charter-apk.json`, the same ward update feed served from this
// PWA's own origin (apps/charter-app/public/charter-apk.json, rewritten by
// each ward release). It carries `apkSha256` and `sizeBytes`, so the digest
// is PINNED here, not merely displayed: the bytes are hashed after download
// and anything that does not match is rejected with a plain-English error
// and never installed.
//
// What that does and does not buy. The APK itself comes from a third-party
// host (Blossom at nostr.download), so a
// tampered or substituted file there cannot get through: its digest would not
// match. The feed, however, is served from the same origin as this code, so
// whoever controls that origin could change both the file and the expected
// digest together. The pin therefore guards against a bad mirror, a truncated
// download and a misconfigured server (a web page answering 200 where the
// APK should be), not against a compromised PWA origin. The digest is still
// shown to the parent so it can be compared with the release notes by eye.
//
// Only nostr.download is used: it answers with `access-control-allow-origin:
// *` and it is the one https origin this page's CSP `connect-src` allows.
// GitHub release downloads redirect to a host with no CORS headers and are
// not in `connect-src`, so a browser could never read them; they are dropped
// here rather than offered as a fallback that cannot work. If several
// nostr.download URLs are listed they are tried in order.

export const FEED_PATH = "/charter-apk.json";

const BLOSSOM_HOST = "nostr.download";

export interface WardApkFeed {
  versionName: string;
  apkSha256: string;
  sizeBytes: number;
  /** https nostr.download URLs only, in listed order, de-duplicated. */
  sources: string[];
}

export interface ReleaseApk {
  bytes: Uint8Array;
  /** SHA-256 of the downloaded bytes (lower-case hex), verified against the feed. */
  sha256: string;
  versionName: string;
}

function hex(buf: ArrayBuffer): string {
  return Array.from(new Uint8Array(buf))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

function isHttps(u: unknown): u is string {
  if (typeof u !== "string") return false;
  try {
    return new URL(u).protocol === "https:";
  } catch {
    return false;
  }
}

function isBlossom(u: string): boolean {
  try {
    return new URL(u).hostname === BLOSSOM_HOST;
  } catch {
    return false;
  }
}

/** Largest APK the feed may claim. The real ward APK is about 28 MB. */
export const MAX_APK_BYTES = 200 * 1024 * 1024;

/** Validate the feed and keep only the sources this origin's CSP allows. */
export function parseWardFeed(o: unknown): WardApkFeed {
  const bad = (why: string): never => {
    throw new Error(
      `The Kintrinsic app's release information could not be read (${why}). Nothing was installed. Wait a moment and retry.`,
    );
  };
  if (typeof o !== "object" || o === null) return bad("not a JSON object");
  const m = o as Record<string, unknown>;
  if (typeof m.versionName !== "string" || m.versionName.length === 0) return bad("no version");
  if (typeof m.apkSha256 !== "string" || !/^[0-9a-f]{64}$/i.test(m.apkSha256)) {
    return bad("no valid checksum");
  }
  if (
    typeof m.sizeBytes !== "number" ||
    !Number.isInteger(m.sizeBytes) ||
    m.sizeBytes <= 0 ||
    m.sizeBytes > MAX_APK_BYTES
  ) {
    return bad("no valid size");
  }
  const sizeBytes = m.sizeBytes;
  const listed: unknown[] = [...(Array.isArray(m.urls) ? m.urls : []), m.url];
  const sources = Array.from(new Set(listed.filter(isHttps).filter(isBlossom)));
  if (sources.length === 0) return bad("no nostr.download address");
  return {
    versionName: m.versionName,
    apkSha256: m.apkSha256.toLowerCase(),
    sizeBytes,
    sources,
  };
}

export async function fetchWardFeed(): Promise<WardApkFeed> {
  let resp: Response;
  try {
    resp = await fetch(FEED_PATH, { cache: "no-store" });
  } catch {
    throw new Error(
      "Could not reach Kintrinsic to look up the phone app. Check the internet connection and retry.",
    );
  }
  if (!resp.ok) {
    throw new Error(
      `The Kintrinsic app's release information was not found (${resp.status}). The site may still be deploying: wait a moment and retry.`,
    );
  }
  let body: unknown;
  try {
    body = await resp.json();
  } catch {
    throw new Error(
      "The Kintrinsic app's release information was not in the expected form. Nothing was installed. Wait a moment and retry.",
    );
  }
  return parseWardFeed(body);
}

/** Download one source and check it against the feed. Throws a parent-readable reason. */
async function downloadVerified(src: string, feed: WardApkFeed): Promise<{ bytes: Uint8Array; sha256: string }> {
  const resp = await fetch(src, { cache: "no-store", redirect: "follow" });
  if (!resp.ok) throw new Error(`download failed (${resp.status})`);
  const type = resp.headers?.get?.("content-type") ?? "";
  if (/text\/html/i.test(type)) throw new Error("the server sent a web page instead of the app");
  const bytes = new Uint8Array(await resp.arrayBuffer());
  if (bytes.length !== feed.sizeBytes) {
    throw new Error(
      `the downloaded app is the wrong size (${bytes.length} bytes, expected ${feed.sizeBytes})`,
    );
  }
  const sha256 = hex(await crypto.subtle.digest("SHA-256", bytes));
  if (sha256 !== feed.apkSha256) {
    throw new Error("the downloaded app does not match its published checksum");
  }
  return { bytes, sha256 };
}

export async function fetchReleaseApk(): Promise<ReleaseApk> {
  const feed = await fetchWardFeed();
  const failures: string[] = [];
  for (const src of feed.sources) {
    try {
      const { bytes, sha256 } = await downloadVerified(src, feed);
      return { bytes, sha256, versionName: feed.versionName };
    } catch (e) {
      const host = new URL(src).hostname;
      failures.push(`${host}: ${e instanceof Error ? e.message : "download failed"}`);
    }
  }
  throw new Error(
    `The Kintrinsic app could not be downloaded and verified, so nothing was installed. ${failures.join("; ")}. Retry in a moment; if it keeps happening, do not install anything and ask for help.`,
  );
}
