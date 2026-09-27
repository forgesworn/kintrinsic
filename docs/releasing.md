# Releasing a build artifact

Kintrinsic ships three build artifacts outside the repo itself — the ward
APK (`android/app`), the guardian ("carrier") APK (`android/carrier`), and
the Linux `.deb` (`linux/`, via `xtask`). Each is announced the same way: a
GitHub Release holds the bytes, Blossom mirrors them, and a signed Nostr
event (kind 30063) is the update signal clients actually watch. This page
describes how one of those artifacts gets from a version bump to an
announced release.

The pipeline is split into two halves on purpose: CI can build and sign, but
only a human publishes and announces.

## The flow

```
tag pushed  ──▶  CI builds + signs  ──▶  DRAFT GitHub Release
(ward-v…,        (release-artifacts.yml)   (never published,
 guardian-v…,                               never "latest")
 linux-v…)
                                                   │
                                                   ▼
                                    maintainer, locally:
                                    publish-release.mjs --from-draft <tag>
                                                   │
                                    verifies the draft end to end,
                                    mirrors to Blossom, signs + relays
                                    the Nostr event, THEN publishes
                                    the GitHub Release
                                                   │
                                                   ▼
                                    published release + Nostr
                                    announcement (what clients see)
```

### 1. Tag it

Bump the version in the source of truth for that artifact
(`android/app/build.gradle.kts` `versionName`, `android/carrier/build.gradle.kts`
`versionName`, or `linux/Cargo.toml` `version`), then push a tag whose
version matches exactly:

```
git tag ward-v0.6.13        # android/app
git tag guardian-v0.1.15    # android/carrier
git tag linux-v0.7.10       # linux/
git push origin <tag>
```

`release-artifacts.yml` triggers on the push (or can be re-run for an
existing tag via `workflow_dispatch`). Its first job validates the tag
against `^(ward|guardian|linux)-v[0-9]+\.[0-9]+\.[0-9]+$` before anything
else runs — a `workflow_dispatch` input is caller-supplied text, so it is
never interpolated into a shell script directly; it only ever reaches one
as this validated step's own output — then checks out `refs/tags/<tag>`
explicitly (failing if the tag doesn't exist) and refuses the run outright
if the tag's version doesn't match the source. A mismatch, or a malformed
tag, never reaches a build.

### 2. CI builds, signs, and drafts

The workflow builds the named artifact (APK jobs run inside a GitHub
Environment gated by a required reviewer; see below), computes its sha256,
and lands a **draft** GitHub Release for the tag with the artifact, a
`SHA256SUMS` file, and a build-provenance attestation
(`actions/attest-build-provenance`) attached. Nothing is published, nothing
is marked "latest", and no Blossom mirror or Nostr event exists yet — a
draft is invisible to everyone but the repo's maintainers.

### 3. The maintainer verifies and publishes

Locally, with the release-signing key available (see
`scripts/release/lib.sh` / the release-key section below):

```
node scripts/release/publish-release.mjs --from-draft ward-v0.6.13 \
  --version-code 44 [--cert <expected-cert-sha256>] [--notes "…"]
```

This:

1. downloads the draft's asset and `SHA256SUMS` with `gh`, and checks the
   downloaded bytes' sha256 against it;
2. runs `gh attestation verify --signer-workflow
   forgesworn/kintrinsic/.github/workflows/release-artifacts.yml
   --source-ref refs/tags/<tag> --deny-self-hosted-runners --format json`
   against the repository — proving the bytes came from exactly that
   workflow, that tag, and a GitHub-hosted runner, not merely "somewhere in
   this repo";
3. reads the attested source commit out of that JSON and, after `git fetch
   origin main`, requires it to be an ancestor of `origin/main` — a build
   from an unmerged or rewritten ref is refused;
4. for an APK, reads the actual signing certificate with `apksigner`
   (including its v3 rotation lineage, if any), refuses a signer equal to
   the debug cert outright, and checks it against the `RELEASE_CERT_SHA256`
   pin (see below) — and, if `--cert` was also given, requires that to
   match too;
5. only once all of that passes: **publishes the draft** to a real GitHub
   Release, verifies the public download URL actually serves it (200,
   following redirects), and only **then** mirrors the artifact to
   Blossom, signs the kind-30063 release event, and publishes it to the
   release relays.

Any failure at any step aborts before anything is announced or (for a
failure before step 5) before the draft is touched — a failed
`--from-draft` run that fails at step 1-4 leaves the draft exactly as CI
left it, and can simply be re-run once the problem is fixed. A failure at
step 5, after the draft has already been published, leaves the GitHub
Release public (by design — it is the primary host) but not yet mirrored
to Blossom or announced on Nostr; re-run the same command with `--resume`
once the problem is fixed. Without `--resume`, `--from-draft` against an
already-published release is refused outright (a plain re-run is never
allowed to silently treat "already published" as "still a draft") — the
refusal message names `--resume`. `--resume` still re-runs every
verification step (sha256, attestation, ancestry, cert pin) in full; it
only skips re-flipping the release to published.

### The release cert pin (`RELEASE_CERT_SHA256`)

`scripts/release/lib.sh` and `scripts/release/release-helpers.mjs` each
carry a `RELEASE_CERT_SHA256` constant, empty until the sysadmin supplies
the fingerprint of the real release key (post-rotation). Until it is set:

- `--from-draft` **refuses to publish any APK channel** ("release cert not
  pinned yet — see docs/releasing.md"). The `charter-deb` channel has no
  cert at all and is unaffected.
- Once it is set, every APK's actual signing cert must equal it exactly, in
  both CI (`android_verify_signing`, lib.sh) and `--from-draft`
  (`assertPinnedCert`, release-helpers.mjs).
- Independently of the pin, a signer equal to the well-known debug cert
  (`DEBUG_CERT_SHA256`) is always refused for real signing material — never
  treated as a trivial "no rotation needed" pass.

`--dry-run` resolves the tag and prints the plan without making any network
calls — safe to run against a real draft to see what would happen.

### Local builds still work

The tag-triggered CI pipeline is not the only way to build these artifacts.
`scripts/publish-deb.sh`, `android/scripts/publish-apk.sh` and
`android/scripts/publish-carrier-apk.sh` still work locally end to end (build
→ GitHub Release → Blossom → Nostr event, all in one run) — CI and the local
scripts share the same build step
(`android/scripts/build-apk.sh` / `build-carrier-apk.sh`), so the two
produce byte-identical output from the same source. `--from-draft` is
additive: it exists so a build that already happened in CI, under the
`release` Environment's reviewer gate, doesn't need repeating by hand.

## The `release` Environment

All three build jobs in `release-artifacts.yml` (ward APK, guardian APK,
and the Linux `.deb`) run inside a GitHub Environment named `release` — the
`.deb` job holds no signing secret, but it still lands a release artifact,
so it gets the same gate. The Environment must be configured (in the
repository's Settings -> Environments, not in the workflow file — GitHub
does not let a workflow declare this for itself) with:

- **Deployment branches and tags restricted** to the three artifact tag
  patterns: `ward-v*`, `guardian-v*`, `linux-v*`. Nothing else may trigger a
  run that reaches this Environment.
- **A required reviewer**, so a signing (or `.deb`-building) job can never
  run unattended — someone has to approve it before secrets are exposed to
  the runner.
- **"Prevent self-review" enabled**, if the repository has more than one
  admin — the person who pushed the tag must not also be the one who
  approves the deployment.

The Environment holds these secrets (names only; their values and custody
are not in this repository):

| Secret | Holds |
|---|---|
| `CHARTER_KEYSTORE_B64` | The release keystore, base64-encoded |
| `CHARTER_KEYSTORE_PASSWORD` | The keystore's password |
| `CHARTER_KEY_ALIAS` | The signing key's alias inside the keystore |
| `CHARTER_KEY_PASSWORD` | The signing key's password |
| `CHARTER_SIGNING_LINEAGE_B64` | An `apksigner` v3 proof-of-rotation lineage file, base64-encoded — **mandatory** alongside the keystore (see below) |

CI decodes the keystore and lineage to a runner-local temp path for the
build, and deletes both again afterwards regardless of whether the build
succeeded.

### Why the lineage is mandatory, not optional

Every ward and guardian phone shipped before the signing-key rotation is
still signed with the project's old debug-era certificate. A release-key
APK built with **no** rotation lineage would pass every on-device check that
only looks at the *release's* declared certificate, and only fail at the
point Android actually tries to install it — on every fielded device at
once. `CHARTER_SIGNING_LINEAGE_B64` being mandatory (both in CI and in
`scripts/release/lib.sh`'s shared signing step) is what turns that into a
build-time failure instead.

A purely local, non-CI build can still skip all of this via the alpha
bridge (`CHARTER_ALPHA_DEBUG_SIGNING=1`, documented in
`android/keystore/README.md`) — that path is unchanged and is never
available in CI, which refuses to start a build if that variable is set on
its environment at all.

Real signing material can also be supplied locally via a git-ignored
`android/signing.properties` instead of the `CHARTER_KEYSTORE_FILE` env var
(Gradle reads either; env wins if both are present — see
`android/app/build.gradle.kts`). `android/scripts/build-apk.sh` and
`build-carrier-apk.sh` resolve `signing.properties` into the same env vars
*before* calling Gradle (`android_resolve_signing_env`, lib.sh), so a
signing.properties-only local build still goes through the mandatory
rotation lineage and cert guards above — it can never silently take the
alpha-bridge code path just because the env var itself was never exported.

## Where the Nostr release key lives

The kind-30063 release event is signed with a separate key from the APK
signing keystore — held privately by the release maintainer, not in this
repository, and not covered by the `release` Environment above. See
`android/keystore/README.md` ("The Nostr release key") for its public trust
anchor and rotation cost.
