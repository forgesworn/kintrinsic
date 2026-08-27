// Pure builders for the D2 release pipeline — no network, no filesystem, so
// `node --test` covers them exactly. The event shape mirrors
// core/crates/charter-verify/src/software_release.rs and
// apps/charter-app/src/release/releaseEvent.ts (golden vector:
// core/crates/charter-testkit/vectors/nostr/software_release.json).

/** Keep in sync with releaseTrust.ts / release_check.rs. */
export const SOFTWARE_RELEASE_KIND = 30063;
export const RELEASE_RELAYS = [
  "wss://relay.trotters.cc",
  "wss://relay.damus.io",
  "wss://nos.lol",
];
export const RELEASE_CHANNELS = ["charter-apk", "mycharter-apk", "charter-deb"];

/** BUD-02 Blossom upload authorization kind. */
export const BLOSSOM_AUTH_KIND = 24242;

const HEX64 = /^[0-9a-f]{64}$/;

/**
 * A URL a device may be told to fetch `sha256` from: https, and the blob at
 * the server ROOT, addressed by its own hash (`https://host/<sha>` or
 * `https://host/<sha>.<ext>`, BUD-01). That path is the one address a
 * Blossom server promises to keep serving. Anything else — in particular a
 * CDN redirect *target* like `media.primal.net/uploads2/a/1e/a8/<sha>` — is
 * an implementation detail that can vanish (primal's did, 2026-08-27: every
 * ward on 0.6.3 sat in a 404-retry loop while the second mirror was fine).
 */
export function isCanonicalBlossomUrl(url, sha256) {
  if (typeof url !== "string" || !HEX64.test(sha256 ?? "")) return false;
  let u;
  try {
    u = new URL(url);
  } catch {
    return false;
  }
  if (u.protocol !== "https:" || u.search || u.hash || u.username || u.password) return false;
  const m = /^\/([0-9a-f]{64})(\.[A-Za-z0-9]{1,8})?$/.exec(u.pathname);
  return m !== null && m[1] === sha256;
}

/**
 * The unsigned kind-30063 template announcing one artifact. Throws on any
 * invalid field — a malformed release event must never reach finalizeEvent.
 */
export function buildReleaseEvent({
  channel,
  versionName,
  versionCode,
  sha256,
  sizeBytes,
  urls,
  certSha256,
  notes,
  createdAt,
}) {
  if (!RELEASE_CHANNELS.includes(channel)) throw new Error(`bad channel: ${channel}`);
  if (typeof versionName !== "string" || versionName.length === 0)
    throw new Error("versionName required");
  if (!Number.isInteger(versionCode) || versionCode <= 0)
    throw new Error("versionCode must be a positive integer");
  if (!HEX64.test(sha256)) throw new Error("sha256 must be 64 lowercase hex chars");
  if (!Number.isInteger(sizeBytes) || sizeBytes <= 0)
    throw new Error("sizeBytes must be a positive integer");
  if (!Array.isArray(urls) || urls.length === 0) throw new Error("at least one url required");
  for (const u of urls) {
    if (typeof u !== "string" || !u.startsWith("https://"))
      throw new Error(`mirror url must be https: ${u}`);
    if (!isCanonicalBlossomUrl(u, sha256))
      throw new Error(`mirror url must be the blob's canonical root address (https://host/<sha>[.ext]): ${u}`);
  }
  if (channel !== "charter-deb" && !HEX64.test(certSha256 ?? ""))
    throw new Error("APK channels require certSha256 (64 lowercase hex chars)");
  if (!Number.isInteger(createdAt) || createdAt <= 0) throw new Error("createdAt required");

  const tags = [
    ["d", channel],
    ["version", versionName],
    ["version_code", String(versionCode)],
    ["x", sha256],
    ["size", String(sizeBytes)],
  ];
  if (certSha256) tags.push(["cert", certSha256]);
  for (const u of urls) tags.push(["url", u]);
  return {
    kind: SOFTWARE_RELEASE_KIND,
    created_at: createdAt,
    tags,
    content: notes ?? "",
  };
}

/**
 * The unsigned BUD-02 upload-authorization template. Signed with the release
 * key and sent as `Authorization: Nostr <base64(signed json)>` on the PUT.
 */
export function buildBlossomAuth({ sha256, createdAt }) {
  if (!HEX64.test(sha256)) throw new Error("sha256 must be 64 lowercase hex chars");
  if (!Number.isInteger(createdAt) || createdAt <= 0) throw new Error("createdAt required");
  return {
    kind: BLOSSOM_AUTH_KIND,
    created_at: createdAt,
    tags: [
      ["t", "upload"],
      ["x", sha256],
      ["expiration", String(createdAt + 600)],
    ],
    content: "Charter release artifact upload",
  };
}
