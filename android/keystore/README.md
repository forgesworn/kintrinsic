# Release keystore — sysadmin-owned deploy secret

This directory intentionally contains no keystore. The release signing key for
`org.forgesworn.charter` is **owned by the sysadmin** as part of the deploy pipeline,
the same way the Hetzner SSH key and other deploy secrets are owned by him
(see the global working agreement: the sysadmin owns keys, we own code).

## How signing works

`android/app/build.gradle.kts` defines a `release` signing config that reads
its material from either source below, **environment variables taking
precedence** over the properties file:

| Field          | Env var                     | `signing.properties` key |
|----------------|------------------------------|---------------------------|
| Keystore file  | `CHARTER_KEYSTORE_FILE`      | `storeFile`                |
| Keystore pass  | `CHARTER_KEYSTORE_PASSWORD`  | `storePassword`            |
| Key alias      | `CHARTER_KEY_ALIAS`          | `keyAlias`                 |
| Key pass       | `CHARTER_KEY_PASSWORD`       | `keyPassword`              |

- **CI / production**: the GitHub Actions deploy pipeline injects the four
  `CHARTER_*` environment variables from repo/org secrets that the sysadmin manages.
  A push to `main` triggers the pipeline, which signs the release build there
  — the private key never needs to touch a developer machine.
- **Local one-off release build** (rare — normally you don't need this): drop
  a `signing.properties` file at the `android/` root (git-ignored, see
  `android/.gitignore`) with the four properties above, pointing `storeFile`
  at a keystore path (e.g. one placed in this `keystore/` directory, which is
  also git-ignored). The sysadmin would need to hand you the keystore file and
  passwords out-of-band for this to work.
- **Local dev / debug**: if neither source is present, `assembleDebug` keeps
  working against the normal Android debug key and **`assembleRelease` fails**
  with a clear error naming the four variables, instead of silently producing
  a mis-signed artifact.

  This page claimed that for months while it was not true (S3, review
  2026-08-07): both `app` and `carrier` quietly fell back to the default debug
  key, and the carrier had no override path at all. Fixed — the fallback is
  gone from both, and the claim above is now enforced by the build.

## The alpha bridge — `CHARTER_ALPHA_DEBUG_SIGNING=1`

Ward phones already in the field pin signing continuity to the **debug**
certificate, and Android only accepts same-signature updates. Until the
signing identity is rotated (see below), there has to be a way to cut an
update those phones will accept. That is this variable, and only this
variable:

```
CHARTER_ALPHA_DEBUG_SIGNING=1 ./scripts/publish-apk.sh
CHARTER_ALPHA_DEBUG_SIGNING=1 ./scripts/publish-carrier-apk.sh
```

It is off by default, warns loudly at build and at publish, and is a debt
rather than a policy — an alpha-only bridge that exists solely so field
devices can accept an update before the signing identity is rotated. Its
security trade-offs and the exact reason it must not outlive alpha are
documented privately (not in this public repo); the mitigating controls that
remain in force regardless are the archive-sha256 pin and the signing-cert
continuity check enforced in `UrlStager`/`ApkInstallOps`.

**Delete this bridge at the rotation.** Both build files and both publish
scripts reference it by name, so `grep -rn CHARTER_ALPHA_DEBUG_SIGNING android/`
finds every site.

## Rotating to a real key — what it costs

Not a re-install. Every fielded phone moves to the new key **in place**, via
an APK Signature Scheme v3 proof-of-rotation lineage (the
android-signing-rotation plan, 2026-09-27): a lineage built from the OLD
(debug) key plus the NEW release certificate, using
`scripts/release/MakeLineage.java`. Android's `PackageManagerService`
honours the lineage from API 28, so a v3-rotated update installs over the
debug-signed app already on the device — the installed signer only needs to
be an ancestor in the incoming lineage, which the debug key is. No factory
reset, no re-provisioning, no re-pairing, and no private key ever changes
hands: the lineage file is public data (it names two certificates, not a
private key), built once by whoever holds both the old debug keystore and
the new release keystore, and from then on carried alongside the release
keystore itself.

Because the signer digest is also pinned on-device, rotating still needs:

1. The sysadmin supplies keystore + the four `CHARTER_KEYSTORE_*` values, and
   the lineage as `CHARTER_SIGNING_LINEAGE_B64`, to CI (see
   `docs/releasing.md`). CI re-signs every release build with the lineage
   before it is ever published (`android_sign_rotated`, `scripts/release/lib.sh`).
2. `apps/charter-app/public/.well-known/assetlinks.json` must be updated with
   the **new** cert fingerprint as a SECOND entry, alongside the debug one.
   `publish-apk.sh` refuses to publish when the APK's actual signing cert is
   not listed there, so this cannot be forgotten — but it does mean the
   first post-rotation publish fails until the file is updated. That failure
   is the feature.
3. Each phone then takes the rotated release the normal way — the guardian's
   "Update" button, or the carrier's self-update — and Android accepts it
   because of the lineage. No manual per-phone re-install step.
4. Remove `CHARTER_ALPHA_DEBUG_SIGNING` from both `build.gradle.kts` files and
   both publish scripts, once the whole fleet has rotated.

No keystore, passphrase, or certificate fingerprint is generated, guessed, or
recorded in this repo. If you ever see a `.jks`, `.keystore`, or
`signing.properties` staged in git, stop — it should never be committed.

## What Task 4 needs from the sysadmin

`apps/charter-app/public/.well-known/assetlinks.json` (the Digital Asset
Links file that authorizes `org.forgesworn.charter` as a verified App Link
handler / WebUSB-provisioning trust anchor) needs the **release certificate's
SHA-256 fingerprint** — a public value derived from the signing cert, safe to
publish. It is **not** the keystore password or private key.

**Today it holds the DEBUG certificate's fingerprint**, matching the phones
actually in the field — correct for what we ship, and it is why the one-scan
QR pairing verifies at all. It is not fabricated and must not be hand-edited
to a guessed value: `publish-apk.sh` compares it against the APK's real
signing cert on every publish and refuses to ship a mismatch.

The sysadmin can produce it once the release keystore exists, via either:

- `keytool -list -v -keystore <release.jks> -alias <alias>` → read the
  `SHA256:` line under "Certificate fingerprints", or
- If using **Play App Signing**, the fingerprint shown in the Play Console
  under App integrity → App signing key certificate.

That fingerprint must come from the sysadmin's actual keystore. Do not fabricate,
guess, or reuse the debug keystore's fingerprint for `assetlinks.json` — a
wrong value silently breaks App Link / WebUSB trust verification on real
devices.

## The Nostr release key (D2 update channel)

Separate from the APK signing keystore above: releases are also announced as
**signed Nostr events** (kind 30063) so the apps can learn about updates from
relays + Blossom instead of the website (D2, internal design spec 2026-08-12-d2-release-events).

- **Secret:** held privately by the release maintainer, never in the repo.
  Generated by `scripts/release/new-release-key.mjs`, which refuses to
  overwrite an existing key. Custody details are kept out of this public repo.
- **Public trust anchor**, compiled into all three clients:

  ```
  RELEASE_PUBKEY_HEX = 11ecfbc95f61796b0b0c5156a24edb324b0ea5e69ff659990b99ebe2f44043a0
  npub               = npub1z8k0hj2lv9ukkzcv29t2ynkmxf9saf0xnlm9nxgtn8479azqgwsqefjk0s
  ```

- **Rotation cost:** every client pins this pubkey at compile time, so
  rotating means shipping new versions of the carrier APK, ward APK, and the
  Linux deb through the OLD key's channel first, then switching. Plan it like
  the APK cert rotation above: deliberate, all-fleet, one-way.
- The event-signing key and the APK-signing keystore are independent trust
  chains on purpose — the release event names the APK's sha256 **and** its
  signing-cert digest, so an attacker needs both keys to ship a malicious
  update through the D2 channel.

## Alpha note

We're in alpha; the key ownership model above (sysadmin-owned, pipeline-only)
is the current decision but may change before a production release (e.g. a
move to Play App Signing with the sysadmin as the upload-key holder). Treat this
doc as reflecting today's decision, not a permanent architecture.
