# QR setup for a ward phone

The cable setup asks a parent for a computer, a Chromium browser, a USB cable,
developer options and USB debugging, all on top of the factory reset that
Android itself requires before an app can become Device Owner. QR setup keeps
the reset and drops the rest: Android's own setup wizard scans a code shown on
the guardian's screen, downloads the ward app, makes it Device Owner and hands
it the pairing link.

The cable setup stays as the alternative.

## The journey

1. The guardian opens Family, Add child, Phone, then **Set up with a QR code**
   (the recommended choice; the cable setup moves under "Other ways").
2. The guardian app shows the steps and a QR code:
   - factory-reset the child's phone (Settings, System, Reset), or start with
     a new one;
   - on the first Welcome screen, tap the same blank spot six times;
   - connect to Wi-Fi if asked, and scan this code.
3. Android downloads the ward app, checks its signing certificate, makes it
   Device Owner and finishes setup. No Google account is added.
4. The ward app opens on its pairing check: the same six-digit code the
   guardian's screen shows. The child or parent compares the codes and taps
   **Pair with guardian**, exactly as after the cable setup.
5. The ward app asks for **Usage access** (one switch in Settings), because
   only adb can grant that app-op without a tap. Until it is on, the time
   meter cannot run, and the ward app says so plainly.
6. The guardian names the device and taps **Govern this device**, as today.

## The QR payload

A JSON object, the standard Android Device Owner provisioning extras
(`android.app.extra.PROVISIONING_*`):

| Key | Value |
|---|---|
| `PROVISIONING_DEVICE_ADMIN_COMPONENT_NAME` | `org.forgesworn.charter/.admin.CharterDeviceAdminReceiver` |
| `PROVISIONING_DEVICE_ADMIN_PACKAGE_DOWNLOAD_LOCATION` | the Blossom URL from `charter-apk.json` (`urls[]` entry on nostr.download, else `url`); https only |
| `PROVISIONING_DEVICE_ADMIN_SIGNATURE_CHECKSUM` | `certSha256` from `charter-apk.json`, hex decoded and re-encoded as URL-safe base64 without padding |
| `PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED` | `true`, so the camera, phone and the rest stay |
| `PROVISIONING_SKIP_ENCRYPTION` | `false` |
| `PROVISIONING_ADMIN_EXTRAS_BUNDLE` | `{ "org.forgesworn.charter.PAIRING_LINK": "<bunker:// link>" }` |
| `PROVISIONING_WIFI_SSID`, `PROVISIONING_WIFI_PASSWORD`, `PROVISIONING_WIFI_SECURITY_TYPE` | only if the guardian typed them in; never stored |

The pairing link is the same `bunker://` link the cable setup and the pairing
QR already carry, generated the same way.

The feed is fetched from the guardian app's own origin (`/charter-apk.json`)
at the moment the code is shown, so the code always names the current release.
Android checks the certificate checksum before it installs; a code for a
build signed by anyone else is refused by the phone itself.

## The ward app

- Android 12 and later require two activities for QR provisioning, both
  protected by `android.permission.BIND_DEVICE_ADMIN`:
  - `ACTION_GET_PROVISIONING_MODE`: return
    `PROVISIONING_MODE_FULLY_MANAGED_DEVICE` (never a work profile).
  - `ACTION_ADMIN_POLICY_COMPLIANCE`: apply the Device Owner baseline the
    cable path applies after `dpm set-device-owner`, then return `RESULT_OK`.
- Read `PROVISIONING_ADMIN_EXTRAS_BUNDLE` (delivered to both activities and to
  `onProfileProvisioningComplete`), take the pairing link, and hold it for the
  pairing check. **Never pin it without the on-screen confirmation**: the
  link is attacker-supplyable here exactly as anywhere else (see
  `MainActivity.showPairingCheck`).
- A link that is missing or will not parse leaves the ward app at its normal
  unpaired screen, where pairing by scan or paste still works.
- Usage access: detect it (`AppOpsManager.OPSTR_GET_USAGE_STATS`), and while it
  is off show one line and a button that opens
  `Settings.ACTION_USAGE_ACCESS_SETTINGS`. The cable path keeps granting it by
  adb, so this only shows when it is needed.

## Guardian app

- The QR is built in a pure function (`provision/qrPayload.ts`) from the
  feed and the pairing link, with tests against a fixed feed.
- Wi-Fi fields are optional, explained ("only needed if the phone can't find
  your Wi-Fi itself"), held in component state only, and cleared when the
  screen closes.
- If the feed cannot be fetched or is missing `certSha256`, say so and offer
  the cable setup; never show a code that names no certificate.

## Hardware test

1. Factory-reset a spare phone. On Welcome, tap six times; scan the code.
2. Expected: Android installs the ward app and finishes setup with no account;
   the ward app shows the pairing check with the guardian's code.
3. Pair, switch on Usage access when asked, govern the device, and check the
   first schedule applies.
4. Repeat once on the Samsung tablet.
