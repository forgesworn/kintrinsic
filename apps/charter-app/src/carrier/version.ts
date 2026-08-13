// Is the Kintrinsic shell around this page up to date?
//
// Why this is not obvious: Kintrinsic's Android app is a WebView shell on the
// live site. Deploying the PWA changes what is INSIDE it; the shell's own
// Kotlin — which composes notifications — only changes when a new APK is
// installed. On 2026-08-06 that gap cost a real test round: the ward-naming
// had shipped, the page was current, and the notification still said "Your
// ward", with nothing anywhere able to say why.
//
// Reuses `parseManifest`/`UpdateManifest` from ../store/updateCheck — the
// Kintrinsic manifest the deploy publishes has the identical shape to the
// ward's, so a second parser would be two things to keep in step.

import { fetchAllReleaseManifests } from "../release/fetchReleases";
import { parseManifest } from "../store/updateCheck";
import type { UpdateManifest } from "../wire/types";

/** Published beside the APK by `android/scripts/publish-carrier-apk.sh`. */
export const MYCHARTER_MANIFEST_PATH = "/mycharter-apk.json";

/** Where a guardian gets or updates the app — the one canonical place. */
export const MYCHARTER_DOWNLOAD_URL = "https://charter.signet.you/download.html#android";

/** What the shell reports about itself via the bridge's `version()`. */
export interface CarrierVersion {
  versionName: string;
  versionCode: number;
}

/**
 * What asking the shell for its version produced.
 *
 * The distinction between `"absent"` and `"unreadable"` is load-bearing, and
 * getting it wrong is what told decented a freshly-installed 0.1.5 was out of
 * date (2026-08-06): the ORIGINAL code collapsed both to `null` and reported
 * "out of date" for either. But a shell that HAS the method is necessarily at
 * least the release that added it — whatever went wrong reading it, "out of
 * date" is the one answer we know to be false.
 */
export type CarrierVersionReading =
  /** No `version` on the bridge — the shell predates it, so it IS behind. */
  | { kind: "absent" }
  /** The method is there but did not yield a usable answer. */
  | { kind: "unreadable" }
  | { kind: "ok"; version: CarrierVersion };

export type KintrinsicVersionState =
  /** Not running inside the app at all (an ordinary browser tab). */
  | { kind: "not-carrier" }
  /** Inside the app, but it predates version reporting — so it IS behind. */
  | { kind: "too-old-to-say" }
  /** New enough to have the method, but it would not answer. Never "behind". */
  | { kind: "installed-unreadable" }
  /** Inside the app and behind the published build. */
  | { kind: "behind"; installed: CarrierVersion; latest: UpdateManifest }
  /** Inside the app and current. */
  | { kind: "current"; installed: CarrierVersion }
  /** Inside the app, but we could not reach the manifest to compare. */
  | { kind: "unknown"; installed: CarrierVersion };

/**
 * Parse the bridge's `version()` payload. Anything malformed is treated as
 * "no answer" (null) rather than a partial one — a shell that cannot say what
 * it is must not be reported as current.
 */
export function parseCarrierVersion(raw: unknown): CarrierVersion | null {
  if (typeof raw !== "string") return null;
  let o: unknown;
  try {
    o = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof o !== "object" || o === null) return null;
  const m = o as Record<string, unknown>;
  if (typeof m.versionName !== "string" || m.versionName.length === 0) return null;
  if (typeof m.versionCode !== "number" || !Number.isInteger(m.versionCode) || m.versionCode <= 0)
    return null;
  return { versionName: m.versionName, versionCode: m.versionCode };
}

/**
 * The whole decision, as a pure function of two readings.
 *
 * `installed === null` means the shell exposed no usable `version()`. That is
 * NOT "unknown, say nothing" — a shell without it necessarily predates the
 * release that added it, so it is genuinely behind and the guardian should be
 * told. (This is the opposite call from `updateCheck.updateAvailable`, which
 * offers nothing for an unreporting WARD device; there, silence is right
 * because the device may be newer than we can tell. Here the absence itself
 * dates the shell.)
 *
 * A missing manifest never claims anything — an offline guardian is told we
 * could not check, not that they are out of date.
 */
export function myCharterVersionState(
  inCarrier: boolean,
  reading: CarrierVersionReading,
  latest: UpdateManifest | null,
): KintrinsicVersionState {
  if (!inCarrier) return { kind: "not-carrier" };
  // ONLY a missing method dates the shell. A method that would not answer
  // proves the opposite — it exists, so the shell is at least the release
  // that added it — and must never be reported as behind.
  if (reading.kind === "absent") return { kind: "too-old-to-say" };
  if (reading.kind === "unreadable") return { kind: "installed-unreadable" };
  const installed = reading.version;
  if (!latest) return { kind: "unknown", installed };
  return installed.versionCode < latest.versionCode
    ? { kind: "behind", installed, latest }
    : { kind: "current", installed };
}

/**
 * Ask the shell what it is.
 *
 * Called as `carrier.version()`, ON the bridge object — NEVER detached into a
 * local (`const v = carrier.version; v()`). Android's `addJavascriptInterface`
 * binds an injected method to its object, so a detached call throws, and the
 * original code's `catch` turned that into "no version", which the caller then
 * read as "out of date" on a brand-new install. `provisionCarrier` and
 * `pushCarrierRoster` were always right about this; this function was the odd
 * one out.
 */
export function readCarrierVersion(): CarrierVersionReading {
  const carrier = window.CharterCarrier;
  if (!carrier || typeof carrier.version !== "function") return { kind: "absent" };
  try {
    const parsed = parseCarrierVersion(carrier.version());
    return parsed ? { kind: "ok", version: parsed } : { kind: "unreadable" };
  } catch {
    return { kind: "unreadable" };
  }
}

/** Fetch + parse the Kintrinsic manifest; null on any failure (fail-quiet).
 *
 *  D2: the signed relay announcement (pinned release key, absolute Blossom
 *  mirrors) is preferred; the origin JSON is the unsigned fallback until D3.
 *  The relay winner is a ReleaseManifest, so `behind` state can offer a real
 *  in-shell install (bridge `installUpdate`) instead of a browser link. */
export async function fetchKintrinsicManifest(
  fetcher: typeof fetch = fetch,
  releases: typeof fetchAllReleaseManifests = fetchAllReleaseManifests,
): Promise<UpdateManifest | null> {
  try {
    const { carrier } = await releases();
    if (carrier) return carrier;
  } catch {
    // fall through to the origin manifest
  }
  try {
    const res = await fetcher(MYCHARTER_MANIFEST_PATH, { cache: "no-store" });
    if (!res.ok) return null;
    return parseManifest(await res.json());
  } catch {
    return null;
  }
}
