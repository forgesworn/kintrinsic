// Self-update availability (#44): read the site's artifact manifest and
// compare against what a device reports in STATUS. Fail-quiet — a missing or
// malformed manifest means "no update to offer", never an error surface.

import type { UpdateManifest } from "../wire/types";

/** Where the deploy publishes the artifact + manifest (same origin as the PWA). */
export const MANIFEST_PATH = "/charter-apk.json";
export const APK_PATH = "/charter-latest.apk";
/** The Linux artifact manifest, published beside the .deb by publish-deb.sh. */
export const DEB_MANIFEST_PATH = "/charter-deb.json";

export function parseManifest(o: unknown): UpdateManifest | null {
  if (typeof o !== "object" || o === null) return null;
  const m = o as Record<string, unknown>;
  if (typeof m.versionName !== "string" || m.versionName.length === 0) return null;
  if (typeof m.versionCode !== "number" || !Number.isInteger(m.versionCode) || m.versionCode <= 0)
    return null;
  if (typeof m.apkSha256 !== "string" || !/^[0-9a-f]{64}$/.test(m.apkSha256)) return null;
  if (typeof m.certSha256 !== "string" || !/^[0-9a-f]{64}$/.test(m.certSha256)) return null;
  if (typeof m.sizeBytes !== "number" || m.sizeBytes <= 0) return null;
  if (typeof m.builtAt !== "string") return null;
  if (m.path !== undefined && (typeof m.path !== "string" || !m.path.startsWith("/"))) return null;
  if (m.url !== undefined && (typeof m.url !== "string" || !m.url.startsWith("https://"))) return null;
  return {
    ...(typeof m.url === "string" ? { url: m.url } : {}),
    ...(typeof m.path === "string" ? { path: m.path } : {}),
    versionName: m.versionName,
    versionCode: m.versionCode,
    apkSha256: m.apkSha256,
    certSha256: m.certSha256,
    sizeBytes: m.sizeBytes,
    builtAt: m.builtAt,
  };
}

/**
 * An update is offered only when the device REPORTS a version and it is
 * strictly behind the manifest. An absent `deviceVersionCode` (older app that
 * doesn't report yet) offers nothing — we can't tell it's behind, and the
 * guardian tapping "update" into the void would be a lie.
 */
export function updateAvailable(
  manifest: UpdateManifest | null,
  deviceVersionCode: number | undefined,
): boolean {
  if (!manifest || deviceVersionCode === undefined) return false;
  return deviceVersionCode < manifest.versionCode;
}

/** Fetch + parse the manifest; null on any failure (fail-quiet). */
export async function fetchManifest(fetcher: typeof fetch = fetch): Promise<UpdateManifest | null> {
  try {
    const res = await fetcher(MANIFEST_PATH, { cache: "no-store" });
    if (!res.ok) return null;
    return parseManifest(await res.json());
  } catch {
    return null;
  }
}

/**
 * What a paired Linux machine could be running. Deliberately a SEPARATE, much
 * smaller shape than the APK manifest: a .deb carries no signing-cert digest,
 * and Kintrinsic can't remotely install one yet — so this exists only to answer
 * "is that laptop behind?", not to authorise anything.
 *
 * `versionCode` is derived from the semver by publish-deb.sh with the same rule
 * charterd uses (major*10000 + minor*100 + patch), so the ONE numeric
 * comparison in `updateAvailable` serves both platforms.
 */
export interface DebManifest {
  versionName: string;
  versionCode: number;
  /** Blossom URL of the .deb (D3). The console only uses this manifest for the
   *  version comparison — the download itself is the front door / charterd's
   *  relay channel — so neither `url` nor `path` is required here. */
  url?: string;
  /** Legacy same-origin path (pre-D3). */
  path?: string;
  sizeBytes: number;
  builtAt: string;
}

export function parseDebManifest(o: unknown): DebManifest | null {
  if (typeof o !== "object" || o === null) return null;
  const m = o as Record<string, unknown>;
  if (typeof m.versionName !== "string" || m.versionName.length === 0) return null;
  if (typeof m.versionCode !== "number" || !Number.isInteger(m.versionCode) || m.versionCode <= 0)
    return null;
  if (m.path !== undefined && (typeof m.path !== "string" || !m.path.startsWith("/"))) return null;
  if (m.url !== undefined && (typeof m.url !== "string" || !m.url.startsWith("https://"))) return null;
  if (typeof m.sizeBytes !== "number" || m.sizeBytes <= 0) return null;
  if (typeof m.builtAt !== "string") return null;
  return {
    versionName: m.versionName,
    versionCode: m.versionCode,
    ...(typeof m.url === "string" ? { url: m.url } : {}),
    ...(typeof m.path === "string" ? { path: m.path } : {}),
    sizeBytes: m.sizeBytes,
    builtAt: m.builtAt,
  };
}

/** Fetch + parse the Linux manifest; null on any failure (fail-quiet). */
export async function fetchDebManifest(
  fetcher: typeof fetch = fetch,
): Promise<DebManifest | null> {
  try {
    const res = await fetcher(DEB_MANIFEST_PATH, { cache: "no-store" });
    if (!res.ok) return null;
    return parseDebManifest(await res.json());
  } catch {
    return null;
  }
}
