// The QR code Android's own setup wizard scans to set a ward phone up from a
// factory reset (docs/qr-setup.md): the standard Device Owner provisioning
// extras, built from the release feed (`/charter-apk.json`) and the same
// `bunker://` pairing link the cable setup fires at the phone.
//
// Pure on purpose: the screen fetches the feed and owns the Wi-Fi fields; this
// only turns them into the payload, or refuses. A code that pins no APK
// checksum is never produced — the checksum is the phone's one defence against
// installing somebody else's build as its Device Owner.
//
// Why the PACKAGE checksum and not the signature checksum: the ward APK is
// signed with a v3 rotation lineage whose oldest certificate is the alpha
// debug key. ManagedProvisioning checks SIGNATURE_CHECKSUM against the
// GET_SIGNATURES view of the package, which for a rotated APK is that OLDEST
// certificate — so the feed's `certSha256` (the current signer) never matches,
// and pinning the debug key would trust anything that key ever signed. The
// SHA-256 of the exact APK bytes pins this one release and nothing else.

const EXTRA = "android.app.extra.";

export const PROVISIONING_KEYS = {
  component: `${EXTRA}PROVISIONING_DEVICE_ADMIN_COMPONENT_NAME`,
  download: `${EXTRA}PROVISIONING_DEVICE_ADMIN_PACKAGE_DOWNLOAD_LOCATION`,
  checksum: `${EXTRA}PROVISIONING_DEVICE_ADMIN_PACKAGE_CHECKSUM`,
  leaveSystemApps: `${EXTRA}PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED`,
  skipEncryption: `${EXTRA}PROVISIONING_SKIP_ENCRYPTION`,
  adminExtras: `${EXTRA}PROVISIONING_ADMIN_EXTRAS_BUNDLE`,
  wifiSsid: `${EXTRA}PROVISIONING_WIFI_SSID`,
  wifiPassword: `${EXTRA}PROVISIONING_WIFI_PASSWORD`,
  wifiSecurity: `${EXTRA}PROVISIONING_WIFI_SECURITY_TYPE`,
} as const;

export const WARD_ADMIN_COMPONENT = "org.forgesworn.charter/.admin.CharterDeviceAdminReceiver";

/** The admin-extras key the ward app reads its pairing link from. */
export const PAIRING_LINK_EXTRA = "org.forgesworn.charter.PAIRING_LINK";

/** The host the Blossom copy of the ward APK lives on. */
export const BLOSSOM_HOST = "nostr.download";

/** The first ward release with the QR provisioning activities (0.6.17). An
 *  older APK installs but cannot answer the setup wizard, which then fails. */
export const MIN_QR_WARD_VERSION_CODE = 48;

/** The provisioning Wi-Fi types Android accepts for a personal network.
 *  "WPA" is WPA/WPA2-PSK; there is no WPA3-SAE value. */
export type WifiSecurity = "WPA" | "WEP" | "NONE";

export interface WifiDetails {
  ssid: string;
  password?: string;
  /** Defaults to WPA when a password is given, NONE when not. */
  security?: WifiSecurity;
}

export type QrPayloadProblem = "no-checksum" | "old-release" | "no-download" | "no-link";

export class QrPayloadError extends Error {
  constructor(
    readonly problem: QrPayloadProblem,
    message: string,
  ) {
    super(message);
    this.name = "QrPayloadError";
  }
}

function isHttps(u: unknown): u is string {
  if (typeof u !== "string") return false;
  try {
    return new URL(u).protocol === "https:";
  } catch {
    return false;
  }
}

function hostOf(u: string): string | null {
  try {
    return new URL(u).hostname;
  } catch {
    return null;
  }
}

/**
 * Where the phone downloads the ward app from: the `urls[]` entry on the
 * Blossom host (a direct, content-addressed fetch — no redirect hop for the
 * setup wizard's downloader), else `url`. https only.
 */
export function downloadLocation(feed: unknown): string {
  const f = (typeof feed === "object" && feed !== null ? feed : {}) as Record<string, unknown>;
  const urls = Array.isArray(f.urls) ? f.urls : [];
  const blossom = urls.find((u): u is string => isHttps(u) && hostOf(u) === BLOSSOM_HOST);
  const chosen = blossom ?? f.url;
  if (!isHttps(chosen)) {
    throw new QrPayloadError(
      "no-download",
      "The release feed names no https download for the ward app.",
    );
  }
  return chosen;
}

/**
 * The APK's SHA-256 as Android wants it in
 * `PROVISIONING_DEVICE_ADMIN_PACKAGE_CHECKSUM`: the raw 32 bytes, URL-safe
 * base64, no padding. The feed carries it as `apkSha256`, 64 hex characters.
 */
export function packageChecksum(apkSha256: unknown): string {
  if (typeof apkSha256 !== "string" || !/^[0-9a-fA-F]{64}$/.test(apkSha256)) {
    throw new QrPayloadError("no-checksum", "The release feed names no checksum for the ward app.");
  }
  let bin = "";
  for (let i = 0; i < 64; i += 2) bin += String.fromCharCode(parseInt(apkSha256.slice(i, i + 2), 16));
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Refuse a feed whose ward app predates the provisioning activities. */
export function requireQrCapableRelease(versionCode: unknown): void {
  if (typeof versionCode !== "number" || !Number.isInteger(versionCode) || versionCode < MIN_QR_WARD_VERSION_CODE) {
    throw new QrPayloadError(
      "old-release",
      "QR setup needs the next ward release; use the cable setup for now.",
    );
  }
}

/** Build the provisioning payload, or throw a [QrPayloadError]. */
export function buildQrPayload(
  feed: unknown,
  pairingLink: string,
  wifi?: WifiDetails,
): Record<string, unknown> {
  const f = (typeof feed === "object" && feed !== null ? feed : {}) as Record<string, unknown>;
  // The checksum first: a feed without one must never yield a code, even
  // one that is otherwise complete.
  const checksum = packageChecksum(f.apkSha256);
  requireQrCapableRelease(f.versionCode);
  const download = downloadLocation(f);
  if (!pairingLink.startsWith("bunker://")) {
    throw new QrPayloadError("no-link", "There is no pairing link to put in the code.");
  }

  const payload: Record<string, unknown> = {
    [PROVISIONING_KEYS.component]: WARD_ADMIN_COMPONENT,
    [PROVISIONING_KEYS.download]: download,
    [PROVISIONING_KEYS.checksum]: checksum,
    [PROVISIONING_KEYS.leaveSystemApps]: true,
    [PROVISIONING_KEYS.skipEncryption]: false,
    [PROVISIONING_KEYS.adminExtras]: { [PAIRING_LINK_EXTRA]: pairingLink },
  };

  // Taken exactly as typed: spaces are legal in an SSID.
  const ssid = wifi?.ssid ?? "";
  if (ssid !== "") {
    const password = wifi?.password ?? "";
    // The provisioning Wi-Fi config takes the SSID as Android's own
    // WifiConfiguration does: a UTF-8 name inside double quotes.
    payload[PROVISIONING_KEYS.wifiSsid] = `"${ssid}"`;
    payload[PROVISIONING_KEYS.wifiSecurity] = wifi?.security ?? (password ? "WPA" : "NONE");
    if (password) payload[PROVISIONING_KEYS.wifiPassword] = password;
  }
  return payload;
}

/** The exact text the QR code encodes. */
export function qrPayloadText(feed: unknown, pairingLink: string, wifi?: WifiDetails): string {
  return JSON.stringify(buildQrPayload(feed, pairingLink, wifi));
}
