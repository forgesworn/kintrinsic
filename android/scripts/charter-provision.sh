#!/usr/bin/env bash
# Charter phone provisioning — the scripted fallback to the Kintrinsic WebUSB flow.
# Installs the release APK as Device Owner on a factory-fresh GrapheneOS phone.
# Usage: charter-provision.sh path/to/app-release.apk ['bunker://…&token=…']
set -euo pipefail

APK="${1:?usage: charter-provision.sh <app-release.apk> [bunker-uri]}"
BUNKER="${2:-}"
ADMIN="org.forgesworn.charter/.admin.CharterDeviceAdminReceiver"

command -v adb >/dev/null || { echo "adb not found on PATH"; exit 1; }
[ -f "$APK" ] || { echo "APK not found: $APK"; exit 1; }

echo "→ Waiting for the phone (authorize USB debugging on it if prompted)…"
adb wait-for-device

echo "→ Preflight: checking for accounts / extra users…"
if adb shell dumpsys account | grep -q 'Account {'; then
  echo "✗ The phone has an account signed in. Factory-reset with NO accounts and retry." >&2
  exit 2
fi
if [ "$(adb shell pm list users | grep -c 'UserInfo{')" -gt 1 ]; then
  echo "✗ The phone has extra user profiles. Remove them (or factory-reset) and retry." >&2
  exit 2
fi

echo "→ Installing the Charter app…"
adb install -r -g "$APK"

echo "→ Making Charter the device owner…"
if ! adb shell dpm set-device-owner "$ADMIN" | tee /dev/stderr | grep -q 'Success'; then
  echo "✗ set-device-owner failed. The phone must be freshly reset (no accounts, one user)." >&2
  exit 3
fi

# Usage access is app-op-backed, so the app's own DO self-grant
# (`WardenController.ensureUsageAccess`, via `setPermissionGrantState`) is
# refused on some builds — it returns false rather than throwing, leaving the
# daily-limit meter silently unable to accrue. Seen on GrapheneOS/bramble
# (issue #42) and again on GrapheneOS/oriole Android 17 (2026-07-27). The cable
# is the only moment we can settle it, so grant it here and VERIFY the op
# actually flipped rather than trusting the set.
echo "→ Granting usage access (the daily-limit meter)…"
adb shell appops set org.forgesworn.charter android:get_usage_stats allow >/dev/null 2>&1 || true
if adb shell appops get org.forgesworn.charter android:get_usage_stats 2>/dev/null \
    | grep -qE ':[[:space:]]*allow\b'; then
  echo "  ✓ usage access held"
else
  echo "✗ Usage access NOT held — the daily-limit meter cannot accrue on this phone." >&2
  echo "  The app retries each tick and logs loudly; grant it before relying on time limits." >&2
  echo "  While the cable is in, try it by hand and re-run:" >&2
  echo "    adb shell appops set org.forgesworn.charter android:get_usage_stats allow" >&2
  exit 4
fi

if [ -n "$BUNKER" ]; then
  echo "→ Opening the Charter app to pair…"
  adb shell am start -a android.intent.action.VIEW -d "$BUNKER" org.forgesworn.charter
  echo "✓ Provisioned + pairing sent. The phone should appear in Kintrinsic shortly."
else
  echo "✓ Provisioned. Open Kintrinsic → Set up a phone → scan the QR to pair."
fi
