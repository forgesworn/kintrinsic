# Digital Asset Links — Kintrinsic App-Link pairing

`assetlinks.json` pins the SHA-256 fingerprint(s) of the signing cert(s) that
Android trusts to open the verified App Link
`https://charter.mysignet.app/pair` directly in the Kintrinsic app (the
system-camera QR-scan pairing path; the host stays `charter.mysignet.app`
until the DNS cutover to `kintrinsic.app`). A build whose signing cert is **not**
listed here falls back to the `bunker://` scheme intent, which always works — so
this file is a convenience for QR-scan pairing, never load-bearing for the
cable-pair flow (that fires `bunker://` directly).

## Current entries

- **Debug key** (the only entry today): the fingerprint in `assetlinks.json` is
  the debug keystore's cert. The alpha hardware builds are debug-signed, so
  QR-scan pairing verifies for them as-is.

## Adding the release cert (the sysadmin's key) — do this when the release build ships

The release APK is signed with **the sysadmin's release keystore** (a deploy secret;
see `android/keystore/README.md`). Its cert fingerprint is a **public** value,
but we do **not** have it yet and must **never fabricate it**. When the sysadmin
provides it, add it as a SECOND entry in the `sha256_cert_fingerprints` array in
`assetlinks.json` (keep the debug entry so dev builds still verify):

```json
"sha256_cert_fingerprints": [
  "D9:C7:F3:DE:...:42",                 // debug (existing)
  "<REAL RELEASE CERT SHA-256 FROM THE SYSADMIN>"   // release — colon-separated hex
]
```

How the sysadmin gets the value from his release keystore:

```bash
keytool -list -v -keystore <release.jks> -alias <alias> | grep -A1 'SHA256:'
```

…or, if using Play App Signing: Play Console → App integrity → **App signing
key certificate** → the SHA-256 fingerprint.

Then push to `main` — the deploy pipeline serves the updated `assetlinks.json`
from the PWA origin, and the release build's App Link verifies. Until then, a
release build simply uses the `bunker://` fallback for pairing (no breakage).
