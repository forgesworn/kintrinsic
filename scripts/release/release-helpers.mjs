// Pure builders for the D2 release pipeline — no network, no filesystem, so
// `node --test` covers them exactly. The event shape mirrors
// core/crates/charter-verify/src/software_release.rs and
// apps/charter-app/src/release/releaseEvent.ts (golden vector:
// core/crates/charter-testkit/vectors/nostr/software_release.json).

/** Keep in sync with releaseTrust.ts / release_check.rs. No relay run by the
 * project is a default (Kintrinsic is decentralised by design) — these are
 * public relays, the same ones clients poll by default. */
export const SOFTWARE_RELEASE_KIND = 30063;
export const RELEASE_RELAYS = [
  "wss://relay.damus.io",
  "wss://nos.lol",
  "wss://relay.primal.net",
];
export const RELEASE_CHANNELS = ["charter-apk", "mycharter-apk", "charter-deb"];

/**
 * RELEASE_RELAYS plus any operator-added relays from `CHARTER_EXTRA_RELAYS`
 * (comma-separated `wss://` URLs, e.g. a relay the founder runs themselves).
 * Kintrinsic ships with none of its own as a default; this is how one is
 * added back for a publish, without making it load-bearing for every client.
 */
export function releaseRelaysWithExtra(env = process.env) {
  const extra = (env.CHARTER_EXTRA_RELAYS ?? "")
    .split(",")
    .map((r) => r.trim())
    .filter((r) => r.startsWith("wss://"));
  return [...new Set([...RELEASE_RELAYS, ...extra])];
}

/**
 * The public repository whose GitHub Releases are the PRIMARY artifact host.
 * Transport is not the trust anchor (the release-event signature, the
 * artifact sha256 and, for APKs, the signing cert are), so bytes come from
 * the most reliable HTTPS host first and Blossom second.
 */
export const RELEASE_GITHUB_REPO = "forgesworn/kintrinsic";

/** Per-channel GitHub Release naming: one tag per artifact. */
const GITHUB_NAMING = {
  "charter-apk": {
    tagPrefix: "ward-v",
    title: "Ward",
    asset: (v) => `kintrinsic-ward-${v}.apk`,
  },
  "mycharter-apk": {
    tagPrefix: "guardian-v",
    title: "Kintrinsic (guardian)",
    asset: (v) => `kintrinsic-${v}.apk`,
  },
  "charter-deb": {
    tagPrefix: "linux-v",
    title: "Kintrinsic for Linux",
    asset: (v) => `kintrinsic_${v}_amd64.deb`,
  },
};

const VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/;
const TAG_RE = /^(ward|guardian|linux)-v[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/;
const ASSET_RE = /^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$/;
const REPO_RE = /^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/;

/**
 * The GitHub Release coordinates for one artifact: tag (`ward-v0.6.13`),
 * asset file name, human title, and the public download URL. The URL 302s to
 * GitHub's asset CDN; clients that follow https redirects fetch it directly,
 * older ones fall through to the Blossom mirror.
 */
export function githubRelease(channel, version, repo = RELEASE_GITHUB_REPO) {
  const n = GITHUB_NAMING[channel];
  if (!n) throw new Error(`bad channel: ${channel}`);
  if (typeof version !== "string" || !VERSION_RE.test(version))
    throw new Error(`bad version for a release tag: ${version}`);
  if (!REPO_RE.test(repo)) throw new Error(`bad GitHub repo: ${repo}`);
  const tag = `${n.tagPrefix}${version}`;
  const asset = n.asset(version);
  return {
    repo,
    tag,
    asset,
    title: `${n.title} ${version}`,
    url: `https://github.com/${repo}/releases/download/${tag}/${asset}`,
  };
}

/**
 * Is `url` a GitHub Release download address of `repo`
 * (`https://github.com/<repo>/releases/download/<tag>/<asset>`) with one of
 * our per-artifact tags? The only non-Blossom address a release event may
 * name: stable for as long as the release exists, and every client pins the
 * bytes to the event's sha256 whatever the host serves.
 */
export function isGithubReleaseUrl(url, repo = RELEASE_GITHUB_REPO) {
  if (typeof url !== "string") return false;
  let u;
  try {
    u = new URL(url);
  } catch {
    return false;
  }
  if (u.protocol !== "https:" || u.host !== "github.com") return false;
  if (u.search || u.hash || u.username || u.password) return false;
  const prefix = `/${repo}/releases/download/`;
  if (!u.pathname.startsWith(prefix)) return false;
  const rest = u.pathname.slice(prefix.length).split("/");
  return rest.length === 2 && TAG_RE.test(rest[0]) && ASSET_RE.test(rest[1]);
}

/** Order announced download URLs: GitHub Release first, then Blossom, dedup'd. */
export function orderReleaseUrls(urls, repo = RELEASE_GITHUB_REPO) {
  const uniq = [...new Set(urls)];
  return [
    ...uniq.filter((u) => isGithubReleaseUrl(u, repo)),
    ...uniq.filter((u) => !isGithubReleaseUrl(u, repo)),
  ];
}

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
    if (!isCanonicalBlossomUrl(u, sha256) && !isGithubReleaseUrl(u))
      throw new Error(
        `mirror url must be a GitHub Release download or the blob's canonical root address (https://host/<sha>[.ext]): ${u}`,
      );
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
