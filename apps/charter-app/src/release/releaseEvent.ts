// Verify + adapt kind-30063 software-release events into the existing
// UpdateManifest shape, so everything downstream (updateAvailable, the
// update clause, Family.tsx) is reused unchanged. Mirrors
// core/crates/charter-verify/src/software_release.rs — the golden vector
// `nostr/software_release.json` pins both to identical bytes.
//
// Deliberately NO freshness window: a release event stays valid; downgrade
// safety is the caller's strictly-greater version comparison.

import { verifyEvent } from "nostr-tools/pure";
import type { UpdateManifest } from "../wire/types";
import { RELEASE_PUBKEY_HEX, SOFTWARE_RELEASE_KIND, type ReleaseChannel } from "./releaseTrust";

/** An UpdateManifest that always names its download sources. */
export interface ReleaseManifest extends UpdateManifest {
  /** Download URLs (https, ≥1) in event order: the GitHub Release first,
   *  then the content-addressed Blossom mirrors. */
  urls: string[];
}

const HEX64 = /^[0-9a-f]{64}$/;

interface EventShape {
  id: string;
  pubkey: string;
  created_at: number;
  kind: number;
  tags: string[][];
  content: string;
  sig: string;
}

function eventShape(ev: unknown): EventShape | null {
  if (typeof ev !== "object" || ev === null) return null;
  const o = ev as Record<string, unknown>;
  if (
    typeof o.id !== "string" ||
    typeof o.pubkey !== "string" ||
    typeof o.created_at !== "number" ||
    typeof o.kind !== "number" ||
    !Array.isArray(o.tags) ||
    typeof o.content !== "string" ||
    typeof o.sig !== "string"
  ) {
    return null;
  }
  // Rebuild a PLAIN object. nostr-tools caches verification under a symbol
  // key (set by finalizeEvent, copied by object spread); a fresh object
  // guarantees verifyEvent actually recomputes the id and checks the sig.
  return {
    id: o.id,
    pubkey: o.pubkey,
    created_at: o.created_at,
    kind: o.kind,
    tags: o.tags as string[][],
    content: o.content,
    sig: o.sig,
  };
}

function firstTag(tags: string[][], name: string): string | undefined {
  const t = tags.find((t) => t.length >= 2 && t[0] === name);
  return t?.[1];
}

/**
 * Authenticate one event for `channel` against the pinned release key and
 * map it into the manifest shape. Returns null on ANY failure (fail-quiet,
 * like parseManifest). Order matches the Rust verifier: kind → channel →
 * id+sig (verifyEvent does both) → pinned author → shape.
 *
 * For the `charter-deb` channel there is no `cert` tag; the artifact sha256
 * still lands in `apkSha256` so the shared comparator/clause plumbing works
 * (the deb path never reads a cert).
 */
export function releaseFromEvent(
  ev: unknown,
  channel: ReleaseChannel,
  pinnedPk: string = RELEASE_PUBKEY_HEX,
): ReleaseManifest | null {
  const e = eventShape(ev);
  if (!e) return null;
  if (e.kind !== SOFTWARE_RELEASE_KIND) return null;
  if (firstTag(e.tags, "d") !== channel) return null;
  // verifyEvent recomputes the id and checks the schnorr sig in one call.
  if (!verifyEvent(e as Parameters<typeof verifyEvent>[0])) return null;
  if (e.pubkey !== pinnedPk) return null;

  const versionName = firstTag(e.tags, "version");
  if (!versionName) return null;
  const versionCode = Number(firstTag(e.tags, "version_code"));
  if (!Number.isInteger(versionCode) || versionCode <= 0) return null;
  const sha256 = firstTag(e.tags, "x");
  if (!sha256 || !HEX64.test(sha256)) return null;
  const sizeBytes = Number(firstTag(e.tags, "size"));
  if (!Number.isInteger(sizeBytes) || sizeBytes <= 0) return null;
  const urls = e.tags.filter((t) => t.length >= 2 && t[0] === "url").map((t) => t[1]);
  if (urls.length === 0 || urls.some((u) => !u.startsWith("https://"))) return null;
  const cert = firstTag(e.tags, "cert");
  if (cert !== undefined && !HEX64.test(cert)) return null;
  if (channel !== "charter-deb" && cert === undefined) return null;

  return {
    versionName,
    versionCode,
    apkSha256: sha256,
    certSha256: cert ?? "",
    sizeBytes,
    builtAt: new Date(e.created_at * 1000).toISOString(),
    urls,
  };
}

/**
 * The best verified announcement for `channel` among relay-returned events:
 * highest versionCode wins. Relays may disagree (an addressable event is
 * "replaced" per-relay, not globally), so never trust replaceable semantics —
 * compare explicitly.
 */
export function latestRelease(
  evs: unknown[],
  channel: ReleaseChannel,
  pinnedPk: string = RELEASE_PUBKEY_HEX,
): ReleaseManifest | null {
  let best: ReleaseManifest | null = null;
  for (const ev of evs) {
    const r = releaseFromEvent(ev, channel, pinnedPk);
    if (r && (best === null || r.versionCode > best.versionCode)) best = r;
  }
  return best;
}

/**
 * Is `url` the blob's canonical Blossom address for `sha256` —
 * `https://host/<sha>` or `https://host/<sha>.<ext>` (BUD-01)? That is the
 * one path a Blossom server promises to keep serving. A CDN redirect target
 * (`media.primal.net/uploads2/a/1e/a8/<sha>`) is not, and the 0.6.9 event
 * carried one FIRST: primal purged it, and every ward taking `urls[0]` looped
 * on HTTP 404 while the second mirror was fine (2026-08-27).
 */
export function isCanonicalBlossomUrl(url: string, sha256: string): boolean {
  if (!HEX64.test(sha256)) return false;
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return false;
  }
  if (u.protocol !== "https:" || u.search || u.hash || u.username || u.password) return false;
  const m = /^\/([0-9a-f]{64})(\.[A-Za-z0-9]{1,8})?$/.exec(u.pathname);
  return m !== null && m[1] === sha256;
}

/** The public repository whose GitHub Releases are the primary artifact host. */
export const RELEASE_GITHUB_REPO = "forgesworn/kintrinsic";

/**
 * Is `url` a GitHub Release download of our repo with one of the per-artifact
 * tags (`https://github.com/forgesworn/kintrinsic/releases/download/
 * ward-v0.6.13/kintrinsic-ward-0.6.13.apk`)? Mirrors
 * scripts/release/release-helpers.mjs. It 302s to GitHub's asset CDN, so only
 * clients that follow https redirects (ward ≥ 0.6.13, shell ≥ 0.1.15) may be
 * handed it; older ones are named a directly-servable Blossom address.
 */
export function isGithubReleaseUrl(url: string, repo: string = RELEASE_GITHUB_REPO): boolean {
  let u: URL;
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
  return (
    rest.length === 2 &&
    /^(ward|guardian|linux)-v[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/.test(rest[0]) &&
    /^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$/.test(rest[1])
  );
}

/**
 * The first ward app versionCode (0.6.13) whose stager follows https
 * redirects. Earlier wards refuse them, and wards up to 0.6.9 have no
 * Blossom fallback of their own either — a GitHub url in their clause would
 * strand them retrying a 302 forever.
 */
export const WARD_FOLLOWS_REDIRECTS_FROM = 44;
/** The first guardian shell versionCode (0.1.15) that follows https redirects. */
export const SHELL_FOLLOWS_REDIRECTS_FROM = 16;

/**
 * Can EVERY target follow a GitHub download's redirect? Only when each one
 * has reported a versionCode at or above `min`. No targets, or any that has
 * not reported, is "no" — silence must never pick the URL an old client
 * cannot fetch.
 */
export function allFollowRedirects(
  reported: readonly (number | undefined | null)[],
  min: number,
): boolean {
  return reported.length > 0 && reported.every((c) => typeof c === "number" && c >= min);
}

/**
 * The one URL to hand a device (the update clause carries exactly one).
 *
 * With `followsRedirects` (the target is known to be new enough — see
 * allFollowRedirects), our GitHub Release download comes first: the primary
 * host. Otherwise — an older or unknown target — the directly-servable
 * canonical Blossom address comes first, exactly as before GitHub Releases:
 * wards ≤ 0.6.12 and shells ≤ 0.1.14 refuse redirects, and wards ≤ 0.6.9 /
 * shells ≤ 0.1.12 have no fallback beyond the url they are named.
 *
 * Within Blossom, extension-bearing beats bare (the bare form 302s on
 * blossom.primal.net); a CDN redirect target is never preferred over a
 * canonical address (2026-08-27); within a class the event's order stands.
 * Every source is held to `sha256` on the device. Empty input → null.
 */
export function pickInstallUrl(
  urls: readonly string[],
  sha256: string,
  followsRedirects = false,
): string | null {
  if (urls.length === 0) return null;
  const rank = (u: string): number => {
    if (isGithubReleaseUrl(u)) return followsRedirects ? 0 : 3;
    if (!isCanonicalBlossomUrl(u, sha256)) return 4;
    return new URL(u).pathname.includes(".") ? 1 : 2;
  };
  let best = urls[0];
  let bestRank = rank(best);
  for (const u of urls.slice(1)) {
    const r = rank(u);
    if (r < bestRank) {
      best = u;
      bestRank = r;
    }
  }
  return best;
}

/** The self-install source for the guardian's own shell at `shellVersionCode`. */
export function shellInstallUrl(
  urls: readonly string[],
  sha256: string,
  shellVersionCode: number | undefined,
): string | null {
  return pickInstallUrl(
    urls,
    sha256,
    allFollowRedirects([shellVersionCode], SHELL_FOLLOWS_REDIRECTS_FROM),
  );
}
