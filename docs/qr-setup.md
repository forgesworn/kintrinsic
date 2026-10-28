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
3. Android downloads the ward app, checks its SHA-256 against the code, makes
   it Device Owner and finishes setup. No Google account is added.
4. Kintrinsic opens on the phone; if it doesn't, open Kintrinsic from the app
   list. It shows its pairing check: the same six-digit code the guardian's
   screen shows. The child or parent compares the codes and taps
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
| `PROVISIONING_DEVICE_ADMIN_PACKAGE_DOWNLOAD_LOCATION` | the Blossom URL from `charter-apk.json` (the https `urls[]` entry on nostr.download, else `url`); https only |
| `PROVISIONING_DEVICE_ADMIN_PACKAGE_CHECKSUM` | `apkSha256` from `charter-apk.json`, hex decoded and re-encoded as URL-safe base64 without padding |
| `PROVISIONING_LEAVE_ALL_SYSTEM_APPS_ENABLED` | `true`, so the camera, phone and the rest stay |
| `PROVISIONING_SKIP_ENCRYPTION` | `false` |
| `PROVISIONING_ADMIN_EXTRAS_BUNDLE` | `{ "org.forgesworn.charter.PAIRING_LINK": "<bunker:// link>" }` |
| `PROVISIONING_WIFI_SSID`, `PROVISIONING_WIFI_PASSWORD`, `PROVISIONING_WIFI_SECURITY_TYPE` | only if the guardian typed them in; never stored. The SSID is sent exactly as typed, inside double quotes. The security type is `WPA` (WPA/WPA2-PSK), `WEP` or `NONE` |

There is no `SIGNATURE_CHECKSUM`. The ward APK is signed with a v3 rotation
lineage whose oldest certificate is the alpha debug key, and ManagedProvisioning
checks the signature checksum against the package's `GET_SIGNATURES` view,
which for a rotated APK is that oldest certificate. The current signer's
`certSha256` therefore never matches, and pinning the debug key instead would
trust anything that key ever signed. The package checksum pins the exact APK
bytes of this one release.

Provisioning Wi-Fi has no WPA3 (SAE) security type. On a WPA3-only network,
connect the phone to Wi-Fi on its own setup screen before scanning, and leave
the Wi-Fi fields empty.

The pairing link is the same `bunker://` link the cable setup and the pairing
QR already carry, generated the same way.

The feed is fetched from the guardian app's own origin (`/charter-apk.json`)
at the moment the code is shown, so the code always names the current release.
Android checks the package checksum before it installs; any other APK at
that URL is refused by the phone itself.

The guardian app refuses to show a code when the feed's `versionCode` is below
48 (ward 0.6.17, the first with the provisioning activities): an older ward
installs but cannot answer the setup wizard. It says "QR setup needs the next
ward release; use the cable setup for now".

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
- Opening the pairing check does not depend on `onProfileProvisioningComplete`,
  which a fully managed QR setup may never send. While the warden service
  runs, once `Settings.Secure.USER_SETUP_COMPLETE` is 1 and the phone is
  unpaired with a held link, it opens MainActivity (a Device Owner may start
  an activity from the background) and posts a "Finish setting up" notification
  that opens it too; once per process. If the setting is unreadable, it posts
  only the notification. MainActivity offers the held link on create, on a new
  intent and on resume. The receiver's broadcast, if it does arrive, opens it as
  well.
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
- If the feed cannot be fetched, or `apkSha256` is missing or not 64 hex
  characters, say so and offer the cable setup; never show a code that pins
  no checksum.

## Hardware test

1. Factory-reset a spare phone. On Welcome, tap six times; scan the code.
2. Expected: Android installs the ward app and finishes setup with no account;
   the ward app shows the pairing check with the guardian's code.
3. Pair, switch on Usage access when asked, govern the device, and check the
   first schedule applies.
4. Repeat once on the Samsung tablet.
