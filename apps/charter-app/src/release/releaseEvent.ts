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

/** An UpdateManifest that additionally names its Blossom mirrors. */
export interface ReleaseManifest extends UpdateManifest {
  /** Content-addressed mirror URLs (https, ≥1), in event order. */
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

/**
 * The one URL to hand a device (the update clause carries exactly one):
 * canonical Blossom addresses beat anything else, extension-bearing beats
 * bare (blossom.primal.net serves `<sha>.apk` direct-200 where the bare form
 * 302s, and the stagers refuse redirects), and within a class the event's
 * order stands. Falls back to `urls[0]` when nothing is canonical — the
 * device still pins the bytes to `sha256`, so a wrong mirror fails closed.
 * Empty input → null.
 */
export function pickInstallUrl(urls: readonly string[], sha256: string): string | null {
  if (urls.length === 0) return null;
  const rank = (u: string): number => {
    if (!isCanonicalBlossomUrl(u, sha256)) return 2;
    return new URL(u).pathname.includes(".") ? 0 : 1;
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
