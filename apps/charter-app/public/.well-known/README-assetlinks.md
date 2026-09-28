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

`assetlinks.json` now lists **two** fingerprints (android-signing-rotation
plan, 2026-09-27):

- **Debug key** (existing): the fielded debug keystore's cert. Kept so any
  device still running a pre-rotation, debug-signed build continues to
  verify App-Link pairing. Dropped only once the whole fleet has rotated
  (rotation plan step 9).
- **Release key** (new): the sysadmin's release keystore's cert. Rotated
  devices — signed via the v3 lineage built from the old debug key plus this
  new cert (`scripts/release/MakeLineage.java`; see `docs/releasing.md`) —
  verify against this entry. No private key changes hands to add it: the
  fingerprint is public data derived from the sysadmin's certificate.

```json
"sha256_cert_fingerprints": [
  "D9:C7:F3:DE:...:42",   // debug — kept until the fleet has fully rotated
  "4A:78:3A:3E:...:DC"    // release — the sysadmin's key
]
```

How the sysadmin got the value from his release keystore:

```bash
keytool -list -v -keystore <release.jks> -alias <alias> | grep -A1 'SHA256:'
```

…or, if using Play App Signing: Play Console → App integrity → **App signing
key certificate** → the SHA-256 fingerprint.

This file must be live on the PWA origin (`https://charter.mysignet.app/.well-known/assetlinks.json`,
both fingerprints present) **before** any rotated ward or carrier build is
installed anywhere — check with `curl -s
https://charter.mysignet.app/.well-known/assetlinks.json`. Until a device
rotates, it keeps verifying against the debug entry as before; a rotated
device verifies against the release entry.
