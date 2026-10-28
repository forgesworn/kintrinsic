// Self-update availability (#44): read the site's artifact manifest and
// compare against what a device reports in STATUS. Fail-quiet — a missing or
// malformed manifest means "no update to offer", never an error surface.

import type { UpdateManifest } from "../wire/types";
import type { Device } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { wardenSupport } from "../domain/wardenSupport";
import { pickInstallUrl } from "../release/releaseEvent";

/** Where the deploy publishes the artifact + manifest (same origin as the PWA). */
export const MANIFEST_PATH = "/charter-apk.json";
export const APK_PATH = "/charter-latest.apk";
/** The Linux artifact manifest, published beside the .deb by publish-deb.sh. */
export const DEB_MANIFEST_PATH = "/charter-deb.json";

/** An optional `urls` list: absent is fine; present must be https strings. */
function parseUrls(v: unknown): string[] | null | undefined {
  if (v === undefined) return undefined;
  if (!Array.isArray(v) || v.length === 0) return null;
  if (!v.every((u) => typeof u === "string" && u.startsWith("https://"))) return null;
  return v as string[];
}

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
  const urls = parseUrls(m.urls);
  if (urls === null) return null;
  return {
    ...(typeof m.url === "string" ? { url: m.url } : {}),
    ...(urls ? { urls } : {}),
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
 * smaller shape than the APK manifest: a .deb carries no signing-cert digest.
 * It answers "is that laptop behind?" and, from charterd 0.7.12, feeds the
 * Linux `update` clause (its `sha256`); the laptop installs only when the
 * release key's own announcement names the same version and sha256.
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
   *  relay channel — so neither `url`, `urls` nor `path` is required here. */
  url?: string;
  /** Download sources in preference order (GitHub Release first). */
  urls?: string[];
  /** Legacy same-origin path (pre-D3). */
  path?: string;
  /** The .deb's sha256 (lowercase hex). Absent on very old feeds; without it
   *  the guardian can only point at the download page, never sign an update. */
  sha256?: string;
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
  if (m.sha256 !== undefined && (typeof m.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(m.sha256)))
    return null;
  const urls = parseUrls(m.urls);
  if (urls === null) return null;
  return {
    versionName: m.versionName,
    versionCode: m.versionCode,
    ...(typeof m.url === "string" ? { url: m.url } : {}),
    ...(urls ? { urls } : {}),
    ...(typeof m.path === "string" ? { path: m.path } : {}),
    ...(typeof m.sha256 === "string" ? { sha256: m.sha256 } : {}),
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

/**
 * What a Linux device row offers when the laptop is behind the feed:
 *
 * - `install` — charterd ≥ 0.7.12 (`wardenSupport`'s `selfUpdate`) and the
 *   feed names the .deb's sha256 and a download: the guardian taps "Install
 *   update" and signs the Linux `update` clause to this laptop.
 * - `download` — an older charterd (it stores the clause and never reads it),
 *   or a feed without a sha256: point at the download page, as before.
 * - `null` — not a paired laptop, not reporting a version, or up to date.
 *
 * The url is the GitHub Release first: every charterd that reads the clause
 * (≥ 0.7.12) follows its redirect (from 0.7.10). The laptop re-pins whatever it
 * fetches to the clause's sha256 and the announcement's size.
 */
export type LinuxUpdateOffer =
  | { kind: "install"; deb: DebManifest & { sha256: string }; url: string }
  | { kind: "download"; deb: DebManifest }
  | null;

export function linuxUpdateOffer(
  device: Device,
  deviceStatus: Record<string, DeviceStatus>,
  deb: DebManifest | null,
): LinuxUpdateOffer {
  if (!deb || device.platform !== "linux" || device.pairing !== "paired" || !device.devicePubkey)
    return null;
  const reported = deviceStatus[device.devicePubkey]?.appVersionCode;
  if (reported === undefined || reported >= deb.versionCode) return null;
  const capable = wardenSupport([device], deviceStatus, "selfUpdate").tooOld.length === 0;
  const sha256 = deb.sha256;
  if (!capable || !sha256) return { kind: "download", deb };
  const sources = deb.urls ?? (deb.url ? [deb.url] : []);
  const url = pickInstallUrl(sources, sha256, true);
  if (!url) return { kind: "download", deb };
  return { kind: "install", deb: { ...deb, sha256 }, url };
}
