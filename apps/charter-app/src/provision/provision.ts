import { ProvisionError, type AdbSession } from "./webusb";

export type ProvisionStep = "preflight" | "install" | "own" | "grant" | "pair" | "done";
export interface ProvisionProgress { step: ProvisionStep; ok: boolean; detail: string }
export type OnProgress = (p: ProvisionProgress) => void;

const REMOTE_APK = "/data/local/tmp/charter-release.apk";
const ADMIN = "org.forgesworn.charter/.admin.CharterDeviceAdminReceiver";

/**
 * Verify the phone can be made Device Owner: no added accounts and no extra
 * users (the two states `dpm set-device-owner` refuses). GrapheneOS build check
 * is best-effort. Throws a precondition error with a specific fix.
 */
export async function preflight(s: AdbSession): Promise<void> {
  const accounts = (await s.shell("dumpsys account")).stdout;
  if (/Account\s*\{/.test(accounts)) {
    throw new ProvisionError(
      "precondition",
      "This phone already has an account signed in. Factory-reset it and skip all account setup, then try again.",
    );
  }
  const users = (await s.shell("pm list users")).stdout;
  const userCount = (users.match(/UserInfo\{/g) || []).length;
  if (userCount > 1) {
    throw new ProvisionError(
      "precondition",
      "This phone has more than one user profile. Remove the extra profiles (or factory-reset) and try again.",
    );
  }
}

/** Push the release APK and install it. */
export async function installApk(s: AdbSession, apk: Uint8Array, onProgress?: OnProgress): Promise<void> {
  onProgress?.({ step: "install", ok: true, detail: "Copying the Kintrinsic app to the phone…" });
  await s.pushFile(apk, REMOTE_APK);
  const res = await s.shell(`pm install -r -g ${REMOTE_APK}`);
  if (!/Success/.test(res.stdout)) {
    throw new ProvisionError("adb", `Install failed: ${res.stdout.trim() || "unknown error"}`);
  }
}

/** Promote the app to Device Owner. Maps the known refusals to plain fixes. */
export async function setDeviceOwner(s: AdbSession): Promise<void> {
  const res = await s.shell(`dpm set-device-owner ${ADMIN}`);
  if (/Success/.test(res.stdout)) return;
  const out = res.stdout.trim();
  let fix = out;
  if (/already several users|already some accounts|not allowed/i.test(out)) {
    fix = "The phone must be freshly reset with no accounts and no extra users. Factory-reset, skip account setup, and retry.";
  } else if (/already.*device owner|already set/i.test(out)) {
    fix = "This phone is already a Device Owner. If it was a previous Kintrinsic setup, factory-reset first.";
  }
  throw new ProvisionError("adb", `Could not make Kintrinsic the device owner: ${fix}`);
}

/**
 * Grant the usage-access appop the daily-limit meter runs on, and VERIFY it
 * took. GET_USAGE_STATS is an appop, not a runtime permission — `pm install
 * -g` does not cover it, and production's own setPermissionGrantState can
 * return false silently, leaving every time limit silently toothless
 * (bramble issue #42; oriole again 2026-07-27). The cable is the one moment
 * a human is present, so a failed grant must stop the setup loudly rather
 * than hand back a phone that looks managed and meters nothing. Mirrors the
 * same step in the dev-side charter-provision.sh.
 */
export async function grantUsageAccess(s: AdbSession): Promise<void> {
  await s.shell("appops set org.forgesworn.charter android:get_usage_stats allow");
  const check = await s.shell("appops get org.forgesworn.charter android:get_usage_stats");
  if (!/:\s*allow\b/.test(check.stdout)) {
    throw new ProvisionError(
      "adb",
      "The phone refused the usage access Kintrinsic's daily-limit meter needs, so time limits would not count anything. Unplug and replug the phone, then try the setup again.",
    );
  }
}

/**
 * Fire the guardian pairing URI at the phone over the cable — the SAME
 * bunker:// intent the QR flow uses, into MainActivity.handlePairingIntent. The
 * phone then echoes the token on its STATUS heartbeat and Kintrinsic matches it.
 */
export async function pairOverCable(s: AdbSession, bunkerUri: string): Promise<void> {
  // Single-quote for the device shell; the URI has no single quotes.
  const res = await s.shell(`am start -a android.intent.action.VIEW -d '${bunkerUri}' org.forgesworn.charter`);
  if (/Error|Exception/i.test(res.stdout)) {
    throw new ProvisionError("adb", `Could not open the Kintrinsic app to pair: ${res.stdout.trim()}`);
  }
}

/** The whole sequence, reporting progress at each gate. */
export async function runProvision(
  s: AdbSession,
  apk: Uint8Array,
  bunkerUri: string,
  onProgress: OnProgress,
): Promise<void> {
  onProgress({ step: "preflight", ok: true, detail: "Checking the phone…" });
  await preflight(s);
  onProgress({ step: "preflight", ok: true, detail: "Phone is ready." });

  await installApk(s, apk, onProgress);
  onProgress({ step: "install", ok: true, detail: "Kintrinsic installed." });

  onProgress({ step: "own", ok: true, detail: "Making Kintrinsic the device owner…" });
  await setDeviceOwner(s);
  onProgress({ step: "own", ok: true, detail: "Kintrinsic is the device owner." });

  onProgress({ step: "grant", ok: true, detail: "Switching on the time meter…" });
  await grantUsageAccess(s);
  onProgress({ step: "grant", ok: true, detail: "Time meter is on." });

  onProgress({ step: "pair", ok: true, detail: "Pairing with your guardian key…" });
  await pairOverCable(s, bunkerUri);
  onProgress({ step: "pair", ok: true, detail: "Pairing sent." });

  onProgress({ step: "done", ok: true, detail: "Waiting for the phone to appear…" });
}
