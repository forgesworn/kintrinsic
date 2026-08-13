# D2 — Signed Releases over Relays + Blossom Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** All three clients (guardian carrier APK, ward APK, Linux warden) learn about and fetch new releases from Nostr relays + Blossom servers instead of the website, verified against a pinned release key.

**Architecture:** A release becomes one addressable Nostr event (kind 30063, one d-tag per artifact channel) signed by a dedicated release key whose x-only pubkey is compiled into every client. The event's tags carry version, sha256, size, cert digest, and one `url` tag per Blossom mirror. The guardian console queries relays for the latest verified event per channel and adapts it into the existing `UpdateManifest`/`DebManifest` shapes, so all downstream logic (`updateAvailable`, `sendCharterUpdate`, the ward's clause-driven `UrlStager`+`ApkInstallOps` install path) is reused unchanged — only the artifact URL becomes a Blossom URL. The carrier gains a real self-install path (bridge method + PackageInstaller with user confirm); the Linux warden gains its first HTTP client and stages a hash-verified deb locally. Origin JSON feeds keep being written and remain the fallback until D3.

**Tech Stack:** Rust (secp256k1 0.30 via charter-crypto, tokio-tungstenite relay client, NEW: reqwest/rustls on Linux), Kotlin (HttpURLConnection, PackageInstaller), TypeScript (nostr-tools ^2.23, vitest), Node scripts (nostr-tools + global WebSocket) for the publisher.

## Global Constraints

- **Local commits only — NEVER push.** (decented 2026-08-12; private CI costs money.)
- **Kind 31116 is TAKEN** (`CHARTER_DEVICE_RELEASE` = parent-gated unpair). Software releases use **kind 30063** (addressable; aligns with the Zapstore convention).
- **Channels (d-tags):** `charter-apk` (ward), `mycharter-apk` (carrier), `charter-deb` (linux). Exact strings, everywhere.
- **Release relays:** the project relay + `wss://relay.damus.io` + `wss://nos.lol`. The project relay must never be the sole one (decented: end state runs no services of ours).
- **Blossom servers (publisher default, overridable via `CHARTER_BLOSSOM_SERVERS`):** `https://blossom.primal.net,https://blossom.band` — VERIFY LIVE at first real publish; do not bake an unverified server into client-visible copy (no-fabricated-values rule).
- **Never downgrade:** clients act only when event `version_code` is strictly greater than installed/reported. A replayed old event is a no-op.
- **Origin feeds stay:** every publish script keeps writing its JSON feed; console falls back to it when relays return nothing. Removal is D3, not D2.
- **Offline must stay calm:** all new relay/Blossom fetches on the console ride inside the existing `raceRefresh` 10s deadline pattern; failure is quiet (`null`), like `fetchManifest` today.
- Rust gates run from `linux/` and `core/`: `cargo fmt --check`, `clippy`, `cargo test` (mock/real feature discipline). Kotlin/JNI are NOT in CI — run `./gradlew :carrier:testDebugUnitTest` etc. locally.
- Branding stays **Charter** (rename branch is parked; user-facing strings that already say Kintrinsic stay as they are — don't introduce new ones).
- Trust anchors are compiled constants baked from the real ceremony output — never a fabricated placeholder key in a commit that claims to work.
- The wire's `UpdateAppBody.validate()` requires `url.starts_with("https://")` — Blossom URLs satisfy it; no wire change.

---

### Task 1: Release key ceremony script (+ run it)

**Files:**
- Create: `scripts/release/new-release-key.mjs`
- Modify: `android/keystore/README.md` (append a "Nostr release key" section)

**Interfaces:**
- Produces: `~/.charter-release/release-key.hex` (32-byte hex secret, chmod 600) and prints the x-only pubkey hex + npub. Later tasks bake the printed pubkey into `RELEASE_PUBKEY_HEX` constants and the publisher reads the secret file.

- [ ] **Step 1: Write the script**

```js
#!/usr/bin/env node
// scripts/release/new-release-key.mjs — one-time release-key ceremony.
// Writes ~/.charter-release/release-key.hex (0600) and prints the pubkey.
// Refuses to overwrite an existing key: rotation is a deliberate ceremony,
// because every client pins this pubkey at compile time.
import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";
import { mkdirSync, writeFileSync, existsSync, chmodSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const dir = join(homedir(), ".charter-release");
const file = join(dir, "release-key.hex");
if (existsSync(file)) {
  console.error(`refusing to overwrite ${file} — rotation is a ceremony, delete it yourself first`);
  process.exit(1);
}
const sk = generateSecretKey();
const skHex = Buffer.from(sk).toString("hex");
mkdirSync(dir, { recursive: true, mode: 0o700 });
writeFileSync(file, skHex + "\n", { mode: 0o600 });
chmodSync(file, 0o600);
const pk = getPublicKey(sk);
console.log(`release key written to ${file}`);
console.log(`RELEASE_PUBKEY_HEX = ${pk}`);
console.log(`npub               = ${nip19.npubEncode(pk)}`);
```

- [ ] **Step 2: Run it once** — `node scripts/release/new-release-key.mjs` (from repo root; nostr-tools resolves from the root package). Record the printed pubkey hex — it is required input for Tasks 2, 4, 6, 8.
- [ ] **Step 3: Document custody** in `android/keystore/README.md`: where the key lives, that all clients pin the pubkey, that rotation requires a coordinated release of all three clients ("easiest-now custody; harden when marketing starts" — decented 2026-08-10). Include the pubkey hex in the doc.
- [ ] **Step 4: Commit** — `git add scripts/release/new-release-key.mjs android/keystore/README.md && git commit -m "feat(release): nostr release-key ceremony script + custody notes"` (the secret itself lives outside the repo).

---

### Task 2: Rust — kind constant + `verify_software_release` (TDD)

**Files:**
- Modify: `core/crates/charter-primitives/src/kinds.rs` (add `SOFTWARE_RELEASE`)
- Create: `core/crates/charter-verify/src/software_release.rs`
- Modify: `core/crates/charter-verify/src/lib.rs` (export the module)

**Interfaces:**
- Consumes: `charter_primitives::{NostrEvent, PubKey, Sha256Hex}`, `charter_verify::event::{id_is_consistent, signature_is_valid}` — copy the verify ORDER from `charter-verify/src/release.rs:37` (kind/shape → id → sig → pinned author), but NO freshness window: releases stay valid indefinitely; downgrade safety comes from version comparison in the caller.
- Produces:
```rust
pub struct SoftwareRelease {
    pub channel: String,        // the d tag
    pub version_name: String,   // "version" tag, non-empty
    pub version_code: u64,      // "version_code" tag, > 0
    pub sha256: Sha256Hex,      // "x" tag
    pub size_bytes: u64,        // "size" tag, > 0
    pub urls: Vec<String>,      // every "url" tag; each must start with https://; >= 1
    pub cert_sha256: Option<Sha256Hex>, // "cert" tag (required by callers for APK channels)
    pub created_at: u64,
}
pub fn verify_software_release(
    event: &NostrEvent,
    pinned_release_key: &PubKey,
    channel: &str,
) -> Result<SoftwareRelease, SoftwareReleaseError>
```
`SoftwareReleaseError` is a small enum (WrongKind, WrongChannel, BadId, BadSig, WrongAuthor, BadShape(&'static str)).

- [ ] **Step 1: Failing tests first** in `software_release.rs` `#[cfg(test)]`: build a real signed event with `charter_crypto::{schnorr_sign, xonly_pubkey, sha256}` + `charter_proto::canonical::canonical_event_string` (same technique as existing verify tests). Cases: happy path maps all fields; wrong author rejected; tampered `version_code` tag (id mismatch) rejected; bad sig rejected; wrong d-tag rejected; missing `x` rejected; non-https url rejected; zero urls rejected; version_code 0 rejected; uppercase sha rejected (Sha256Hex is lowercase-strict); multiple `url` tags all collected in order.
- [ ] **Step 2:** `cd core && cargo test -p charter-verify software_release` → confirm FAIL (module missing).
- [ ] **Step 3: Implement.** Tag extraction: first `d`/`version`/`version_code`/`x`/`size`/`cert` tag wins; all `url` tags collect. Verify order exactly: `event.kind == kinds::SOFTWARE_RELEASE` → channel match → `id_is_consistent` → `signature_is_valid` → `event.pubkey == *pinned_release_key` → shape parse.
- [ ] **Step 4:** `cargo test -p charter-verify` → PASS; `cargo fmt` + `cargo clippy -p charter-verify -- -D warnings`.
- [ ] **Step 5: Commit** — `feat(verify): kind-30063 software release events verified against a pinned release key`.

---

### Task 3: Cross-stack golden vector (TS producer ↔ Rust consumer)

**Files:**
- Modify: `scripts/gen-nostr-vectors.mjs` (emit a signed software-release fixture with a throwaway TEST key — not the real release key)
- Create: fixture JSON next to the existing `nostr/…` fixtures (follow the exact directory the script already writes to)
- Create: `core/crates/charter-verify/tests/interop_software_release.rs`

**Interfaces:**
- Consumes: Task 2's `verify_software_release`. The fixture embeds: the test pubkey, the event, and the expected parsed fields.
- Produces: a fixture also consumed by Task 4's TS tests, pinning both stacks to identical bytes.

- [ ] **Step 1:** Extend the generator: `finalizeEvent({kind: 30063, tags: [["d","charter-apk"],["version","0.6.9"],["version_code","40"],["x", "<64hex>"],["size","27693181"],["cert","<64hex>"],["url","https://blossom.example/<64hex>"],["url","https://mirror.example/<64hex>"]], content: "test release notes", created_at: 1754900000}, testSk)`. Regenerate vectors.
- [ ] **Step 2:** Rust interop test: load fixture, `verify_software_release` with the fixture's pubkey → assert every field; then flip one tag byte → assert `BadId`.
- [ ] **Step 3:** `cd core && cargo test -p charter-verify --test interop_software_release` → PASS.
- [ ] **Step 4: Commit** — `test(verify): golden vector binds TS-signed release events to the Rust verifier`.

---

### Task 4: TS — release trust constants + event verify/adapt module (TDD)

**Files:**
- Create: `apps/charter-app/src/release/releaseTrust.ts`
- Create: `apps/charter-app/src/release/releaseEvent.ts`
- Create: `apps/charter-app/src/release/releaseEvent.test.ts` (follow the app's existing colocated `*.test.ts` convention)

**Interfaces:**
- Consumes: `verifyEvent` from `nostr-tools/pure`; `UpdateManifest` from `../wire/types` (fields `versionName, versionCode, apkSha256, certSha256, sizeBytes, builtAt, path?`).
- Produces:
```ts
// releaseTrust.ts
export const SOFTWARE_RELEASE_KIND = 30063;
export const RELEASE_PUBKEY_HEX = "<Task 1 ceremony output>";
export const RELEASE_RELAYS = ["wss://relay.trotters.cc", "wss://relay.damus.io", "wss://nos.lol"];
export type ReleaseChannel = "charter-apk" | "mycharter-apk" | "charter-deb";

// releaseEvent.ts
export interface ReleaseManifest extends UpdateManifest { urls: string[] }
export function releaseFromEvent(ev: unknown, channel: ReleaseChannel, pinnedPk?: string): ReleaseManifest | null;
export function latestRelease(evs: unknown[], channel: ReleaseChannel, pinnedPk?: string): ReleaseManifest | null;
```
`releaseFromEvent`: verify kind + d-tag + `verifyEvent` + author === pin → map tags into the manifest shape (`x`→`apkSha256`, `cert`→`certSha256` — for the `charter-deb` channel `cert` is absent, map `x` into `apkSha256` anyway so the shared comparator works, and callers know deb has no cert), `size`→`sizeBytes`, `created_at`→ISO `builtAt`, all `url` tags→`urls` (https-only, ≥1). `latestRelease`: max `versionCode` among verified events (relays may disagree; never trust replaceable semantics alone). `pinnedPk` parameter defaults to `RELEASE_PUBKEY_HEX` (injectable for tests).

- [ ] **Step 1: Failing tests:** sign events with a test key via `finalizeEvent`; mirror Task 2's cases; plus: `latestRelease` picks max versionCode across three events; unverifiable event among them is skipped; load the Task 3 golden fixture and assert identical parsed fields to the Rust expectations.
- [ ] **Step 2:** `cd apps/charter-app && npm test -- release` → FAIL, implement, → PASS. `npm run typecheck` (or the tsc step the deploy workflow runs).
- [ ] **Step 3: Commit** — `feat(app): verify + adapt kind-30063 release events into UpdateManifest shape`.

---

### Task 5: TS — console learns manifests from relays (origin fallback)

**Files:**
- Create: `apps/charter-app/src/release/fetchReleases.ts` (+ colocated test)
- Modify: `apps/charter-app/src/store/store.tsx:1179-1187` (`refetchManifests`)
- Modify: `apps/charter-app/src/carrier/version.ts` (`fetchKintrinsicManifest`)
- Modify: `apps/charter-app/src/store/store.tsx:1292-1314` (`sendCharterUpdate` URL selection)
- Modify: `apps/charter-app/src/screens/Family.tsx` (deb notice link — see step 4)

**Interfaces:**
- Consumes: Task 4's `latestRelease` + `RELEASE_RELAYS`; existing `fetchManifest`/`fetchDebManifest` (unchanged, now the fallback); `SimplePool.querySync` (injectable seam like `fetcher` in updateCheck).
- Produces:
```ts
export interface ReleaseQuery { (relays: string[], filter: object): Promise<unknown[]> } // wraps pool.querySync
export async function fetchAllReleaseManifests(q?: ReleaseQuery): Promise<{
  ward: ReleaseManifest | null, carrier: ReleaseManifest | null, deb: ReleaseManifest | null
}>
```
One relay query for all three channels: `{kinds:[30063], authors:[RELEASE_PUBKEY_HEX], "#d":["charter-apk","mycharter-apk","charter-deb"]}`, then split client-side via `latestRelease`.

- [ ] **Step 1: Failing test** for `fetchAllReleaseManifests` with an injected fake query returning mixed events; and a test that a rejecting query resolves all-null (fail-quiet).
- [ ] **Step 2: Implement**, then wire `refetchManifests`: run `fetchAllReleaseManifests()` alongside the two origin fetches (all inside the existing `raceRefresh` deadline); per channel prefer the relay-verified manifest, else origin. Store the winning manifests exactly where they're stored today so `Family.tsx` reads unchanged.
- [ ] **Step 3:** `sendCharterUpdate`: when the winning ward manifest has `urls`, sign the clause with `manifest.urls[0]` verbatim (absolute Blossom URL); else keep the existing `new URL(manifest.path ?? APK_PATH, window.location.origin)` build. The wire's https-only validate covers both.
- [ ] **Step 4:** `carrier/version.ts`: `fetchKintrinsicManifest` gains the same relay-first/origin-fallback shape for the `mycharter-apk` channel (keep the injectable-fetcher seam). The Linux deb notice in `Family.tsx:569-574` keeps its front-door link (the front door survives D3 as distribution; the *feed* is what decentralizes here).
- [ ] **Step 5:** `npm test` + `npm run build` green. Commit — `feat(app): update manifests come from signed relay events, origin JSON demoted to fallback`.

---

### Task 6: Publisher — `publish-release.mjs` (sign → Blossom → relays)

**Files:**
- Create: `scripts/release/publish-release.mjs`
- Create: `scripts/release/release-helpers.mjs` (pure, testable) + `scripts/release/release-helpers.test.mjs` (run with `node --test`)
- Modify: `android/scripts/publish-apk.sh`, `android/scripts/publish-carrier-apk.sh`, `scripts/publish-deb.sh` (each gains a final step invoking the publisher; skippable with `CHARTER_RELEASE_EVENT_SKIP=1`)

**Interfaces:**
- Consumes: `~/.charter-release/release-key.hex` (Task 1); artifact path + version fields each shell script already has in variables.
- Produces (helpers):
```js
export function buildReleaseEvent({channel, versionName, versionCode, sha256, sizeBytes, urls, certSha256, notes, createdAt}) // -> unsigned event template
export function buildBlossomAuth({sha256, sizeBytes, createdAt}) // -> unsigned kind-24242 BUD-02 auth template: t=upload, x=<sha>, expiration=createdAt+600
```
CLI: `node scripts/release/publish-release.mjs --channel charter-apk --artifact <path> --version 0.6.9 --version-code 40 [--cert <64hex>] [--notes "…"] [--dry-run]`. Flow: sha256 the artifact → for each server in `CHARTER_BLOSSOM_SERVERS` (default per Global Constraints): signed BUD-02 auth header + `PUT <server>/upload` → then `HEAD <server>/<sha256>` must 200 → collect verified URLs → refuse to continue with zero verified mirrors → `buildReleaseEvent` with those URLs → `finalizeEvent` with the release key → publish to every relay in RELEASE_RELAYS with a hand-rolled `["EVENT",…]`/wait-`["OK",…]` over the global `WebSocket` (Node ≥ 22; 10s timeout per relay; exit non-zero if ALL relays fail, warn if some). `--dry-run` prints the signed event and uploads nothing.

- [ ] **Step 1: Failing `node --test` tests** for both helpers (exact tags, exact expiration arithmetic, cert tag omitted when absent).
- [ ] **Step 2: Implement helpers + CLI.** Keep every network call in small named functions (`uploadToBlossom`, `verifyBlossom`, `publishToRelay`).
- [ ] **Step 3:** `node --test scripts/release/` → PASS; `node scripts/release/publish-release.mjs --channel charter-deb --artifact apps/charter-app/public/charter-latest.deb --version 0.7.6 --version-code 706 --dry-run` prints a valid signed event (verify it round-trips through Task 4's `releaseFromEvent` in a helper test).
- [ ] **Step 4:** Append the invocation to the three publish scripts (deb passes no `--cert`; APK scripts pass `$CERT`; all pass their existing `$VN/$VC/$SHA/artifact` vars). The scripts' origin-feed writing stays untouched.
- [ ] **Step 5: Commit** — `feat(release): publish signed kind-30063 release events + Blossom mirrors from the publish scripts`.

---

### Task 7: Carrier self-update (Kotlin bridge + PackageInstaller + TS button)

**Files:**
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/update/ApkStager.kt` (port of the ward's `UrlStager` — streaming sha256 into `.part`, rename only on match, no redirects, 15s/60s timeouts)
- Create: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/update/SelfUpdater.kt`
- Modify: `android/carrier/src/main/kotlin/org/forgesworn/mycharter/web/CarrierBridge.kt` (two new methods)
- Modify: `android/carrier/src/main/AndroidManifest.xml` (`REQUEST_INSTALL_PACKAGES`)
- Modify: `apps/charter-app/src/carrier/AppVersion.tsx` (+ its test if present)
- Test: `android/carrier/src/test/kotlin/org/forgesworn/mycharter/update/ApkStagerTest.kt` (mirror `UrlStagerTest` cases against a local `HttpURLConnection`-servable fixture)

**Interfaces:**
- Consumes: Task 5's carrier manifest (now carrying `urls`).
- Produces (bridge, BOTH optional + feature-detected — a current page runs inside old shells; call methods ON the object):
```kotlin
@JavascriptInterface fun installUpdate(url: String, sha256: String): String // "started" | error string; kicks a background thread
@JavascriptInterface fun installState(): String // JSON {"phase":"idle|downloading|verifying|waiting-user|failed|done","error":null|string}
```
`SelfUpdater`: stage via `ApkStager` into `context.filesDir/updates/mycharter.apk` → `PackageInstaller` session (`MODE_FULL_INSTALL`, own package) → commit → on `STATUS_PENDING_USER_ACTION` fire the confirm `Intent` (carrier is NOT device-owner; the system dialog is expected UX) → map results into `installState`.
- TS: in `AppVersion.tsx`'s `behind` branch, when `window.CharterCarrier?.installUpdate` exists AND the manifest has `urls`, render an "Update now" button → `carrier.installUpdate(urls[0], apkSha256)` + poll `installState()` for progress text; otherwise keep today's `download.html` link (old-shell fallback).

- [ ] **Step 1:** Write `ApkStagerTest` first → red; port `ApkStager` → green (`cd android && ./gradlew :carrier:testDebugUnitTest`).
- [ ] **Step 2:** Implement `SelfUpdater` + bridge + manifest permission. Version-guard: refuse when the offered sha equals the running APK's own digest or `versionCode <=` current — idempotence.
- [ ] **Step 3:** TS button + feature detection; `npm test`; distinguish "method absent" (old shell) from "call failed" (real error) — the AppVersion footer must never claim "behind" off an unreadable state.
- [ ] **Step 4:** `./gradlew :carrier:assembleDebug` builds clean.
- [ ] **Step 5: Commit** — `feat(carrier): in-app self-update — Blossom download, sha256 pin, PackageInstaller confirm`.

---

### Task 8: Linux — charterd learns releases + stages the deb

**Files:**
- Modify: `linux/crates/charterd/Cargo.toml` (`reqwest = { version = "0.12", default-features = false, features = ["rustls-tls-webpki-roots"] }` — MUST reuse the existing rustls/ring stack, no second TLS)
- Create: `linux/crates/charterd/src/release_check.rs`
- Modify: `linux/crates/charterd/src/runtime.rs` (hook a throttled check into the existing poll cadence; every ~6h of uptime, first check a few minutes after boot)

**Interfaces:**
- Consumes: `charter_verify::software_release::verify_software_release` (Task 2), the existing `RelayTransport::query` (`Filter { kinds: [SOFTWARE_RELEASE], authors: [pin], .. }` — Filter has no d-tag field; filter by channel client-side), `charterd::version::version_code()`.
- Produces:
```rust
pub const RELEASE_PUBKEY_HEX: &str = "<Task 1 ceremony output>";
pub const RELEASE_RELAYS: &[&str] = &["wss://relay.trotters.cc", "wss://relay.damus.io", "wss://nos.lol"];
pub async fn check_and_stage(transport: &dyn RelayTransport, now: u64) -> Option<StagedUpdate>
pub struct StagedUpdate { pub version_name: String, pub version_code: u64, pub path: PathBuf }
```
Behaviour: query → `verify_software_release(ev, pin, "charter-deb")` → max version_code → if `> version_code()`: download (size-capped at 64 MiB, whole-body then `charter_crypto::sha256` — a ~10 MB deb needs no streaming) from `urls` in order until one hash-verifies → write `/var/lib/charter/updates/charter_<versionName>_amd64.deb` (0644) + `ready.json` `{versionName, versionCode, sha256, stagedAt}` → journald log line `charterd: update <versionName> staged at <path> — install with: sudo apt install <path>`. Already-staged same version short-circuits. All errors are logged-and-dropped; never blocks the poll loop (spawn/timeout).

- [ ] **Step 1: Failing tests** in `release_check.rs`: MockRelayTransport serving a valid event → stages (download fn injected as a trait/closure so tests feed bytes without HTTP); wrong-author event → None; equal version → None; hash-mismatched first mirror falls through to second; oversize declared → refused.
- [ ] **Step 2:** `cd linux && cargo test -p charterd release_check` red → implement → green. `cargo fmt --check` + `cargo clippy` + full `cargo test` (mock features) still green.
- [ ] **Step 3:** Wire the throttle into `runtime.rs`; real-build gate: `cargo build --release -p charterd` (real features) compiles.
- [ ] **Step 4: Commit** — `feat(charterd): stage hash-verified deb updates from signed relay events (first HTTP client in the linux tree)`.

---

### Task 9: Release checklist + docs

**Files:**
- Modify: the release-order docs (`site/README.md` §deploy + `docs/superpowers/specs/2026-07-21-charter-self-update-design.md` gets a pointer note) and `internal/specs/2026-08-10-decentralized-stack.md` (mark D2 code-ready state)

- [ ] **Step 1:** Document the new publish order: bump versions → `publish-*.sh` (which now also publish the event + Blossom mirrors) → `sync-front-door-downloads.sh` → commit `public/` + `site/` (push only on decented's word). Note the fallback semantics and the D3 removal criteria (fleet seen updating via relays once).
- [ ] **Step 2: Commit** — `docs(release): D2 publish order — signed events + Blossom, origin feeds as fallback`.

---

### Task 10: End-to-end dry-run + hardware-gate handoff

- [ ] **Step 1:** Full local gate sweep: `core` + `linux` cargo fmt/clippy/test, `apps/charter-app` npm test + build, `android` gradle unit tests + `:carrier:assembleDebug` + `:app:assembleDebug`.
- [ ] **Step 2:** `--dry-run` publish for all three channels off the CURRENT artifacts; pipe each printed event through a small verification snippet using Task 4's module. This proves the publisher/verifier loop without touching relays.
- [ ] **Step 3:** Live smoke WITHOUT artifacts: publish a release event for the *current* (not bumped) deb version to the real relays (harmless: equal version_code = no-op for every client), then query it back via `fetchAllReleaseManifests` in a node snippet. This proves relay round-trip + that public relays accept kind 30063. If a public Blossom server rejects our upload size, record which and adjust the default server list (that's the one live-verification item).
- [ ] **Step 4:** Write the turnkey hardware-round ask for decented (one bundled round, exact steps + expected results): (a) carrier "Update now" self-installs from Blossom on his phone, (b) a ward-phone update clause carrying a Blossom URL installs, (c) the laptop stages the deb and logs the install command. Gate: the NEXT real version bump.
- [ ] **Step 5: Commit** any fixes; update memory (`decentralized-guardian-direction`) with D2 code-ready state.

---

## Self-review notes

- Spec coverage: "release = signed NIP-94-shaped event + blob on Blossom" (Tasks 2–6), "all three clients switch update checks" (5 = console feeds, 7 = carrier install, ward needs no client change — the clause path carries Blossom URLs from Task 5 step 3, 8 = laptop), "origin manifests stay as fallback" (5, 9), "pin the release pubkey" (1, 4, 8), "Android first; Linux deb second" (task order), "app downloads the deb and hands it to the parent" (8).
- Ward client is deliberately untouched: `UrlStager` + cert gate + `UpdateAppBody` already enforce everything; D2 only changes what URL rides in the clause.
- Type consistency: `ReleaseManifest extends UpdateManifest` keeps `Family.tsx`/`updateAvailable` untouched; Rust `SoftwareRelease.version_code: u64` matches `app_version_code: u64` on STATUS.
- Known deferred items (explicitly NOT in D2): tray/console UI surfacing of the staged deb on Linux (journald + ready.json only for now); Blossom mirroring of OLD ward artifacts (RETAIN=4 window keeps living on the origin until D3); first-install distribution stays on the front door.
