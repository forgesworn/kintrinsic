# Charter on GrapheneOS — port spec

> **Status: implementation-grade spec.** Synthesized 2026-07-06 from eight deep-read
> reports over the Linux warden, the PWA, the wire contract, and the hoist analysis,
> grounded in [`concept.md`](concept.md) (architecture) and [`spike.md`](spike.md)
> (capability contract / Go-No-Go gate). Build order is spike-gated:
> **spike green → §1 hoist → §2 android/jni → §3 android/app.**
> All `path:line` citations are to the current tree (pre-hoist); after §1 the nine
> moved crates read `core/crates/…` instead of `linux/crates/…`.
>
> Division of labor (concept.md:45-50, 228-230): **Rust owns decide/verify/crypto,
> single-sourced with the Linux warden. Kotlin owns only DevicePolicyManager
> enforcement.** Nothing trust-critical is reimplemented in Kotlin.

---

## 1. The hoist plan — `core/` extraction

### 1.1 What moves (Phase H1 — pure `git mv`, zero source edits except one fix)

Move **nine** crates from `linux/crates/` to `core/crates/`:

| Crate | Why it moves | Purity evidence |
|---|---|---|
| charter-primitives | kinds, hex newtypes, NostrEvent | deps: serde only (`charter-primitives/Cargo.toml:9-13`) |
| charter-crypto | the single BIP-340/secp256k1 backend | secp256k1 + sha2 only; deterministic sign (`charter-crypto/src/lib.rs:74-85`) |
| charter-proto | REQUEST/GRANT/CLAUSE/STATUS payloads, canonical NIP-01 id | serde/serde_json only |
| charter-schedule | schedule/budget evaluator, ledgers, EnforcerCore | serde, chrono (no `clock` feature), chrono-tz (embedded tzdb) (`charter-schedule/Cargo.toml:9-18`) |
| charter-content | curator list model | serde/serde_json only |
| charter-verify | the 6 grant rules + clause auth | pure given charter-sys traits (`charter-verify/src/verify.rs:9-10`) |
| charter-transport | NIP-44/59, pairing, transport | pure given charter-sys; no `tokio::` symbol in src |
| **charter-sys** | **the port-trait layer itself** | featureless = pure trait/type definitions (`charter-sys/src/lib.rs:17-29`) |
| **charter-testkit** | golden vectors + builders, dev-dep of 7 crates | mock-only, in-memory; vectors resolve via `env!("CARGO_MANIFEST_DIR")` (`charter-testkit/src/golden.rs:9-11`) so they move with the crate, no code change |

**charter-sys and charter-testkit are additions to the concept.md §9 list** (concept.md:206-213
names only seven). The concept list is not closed under the dependency graph: charter-verify,
charter-transport, and charter-testkit all hard-depend on charter-sys
(`charter-verify/Cargo.toml:20`, `charter-transport/src/transport.rs:11`,
`charter-testkit/Cargo.toml:12`), and charter-testkit is a dev-dep of both moving crates and
staying crates (`charterd/Cargo.toml:51`, `charter-webpolicy/Cargo.toml:12`). Do **not**
extract a new "ports" crate instead — hoisting charter-sys whole is a file move; splitting it
is a refactor, and featureless charter-sys is already OS-agnostic (empirically proven: all 9
crates `cargo ndk -t arm64-v8a -t x86_64 check --no-default-features` green on both ABIs,
including the secp256k1-sys and ring C compiles, run 2026-07-06 on toolchain 1.94.1 /
NDK 29.0.14206865).

**The one mandatory source edit — the VT ioctl.** `charter-sys --features real` fails to
cross-compile with exactly two errors: bionic's `ioctl` takes `Ioctl = c_int`, glibc takes
`c_ulong`, and the VT code passes `c_ulong` (`charter-sys/src/effects.rs:1331`, `:1348`).
Fix: type the request constants as `libc::Ioctl` (exists on both, libc-0.2.186
`unix/linux_like/mod.rs:1731`). Zero behavior change on Linux.

**Recommended hygiene in the same commit (non-blocking):** split charter-sys's `real`
feature into `real-relay = [dep:tokio, dep:tokio-tungstenite, dep:futures-util]` and
`real-os = [dep:libc]`, with `real = ["real-relay","real-os"]` for back-compat. `android/jni`
then enables `real-relay` only, keeping the Linux shell-out effects (dead code on Android)
out of the `.so`.

### 1.2 What stays in `linux/`

charterd, charter-ipc (zbus), charter-cli, charter-lock (x11rb), charter-webpolicy, xtask.
(charter-webpolicy is actually pure and could hoist later if Android grows web policy; keep
it in `linux/` per concept for now.)

### 1.3 Phase H2 — extract `core/crates/charter-spine` from charterd

concept.md reuse-lists charterd's spine (concept.md:35-38) but leaves charterd in `linux/`
(concept.md:211-213). That contradiction is resolved here: **extract the spine as a new
crate**, `core/crates/charter-spine`, in a second, separate commit (it is a refactor, not a
file move — keep it out of the zero-change H1 diff). Android must not path-depend on a crate
under `linux/`, and reimplementing the broker in the jni crate would fork the security spine.

Modules that move from `linux/crates/charterd/src/` into charter-spine (all generic over
`SystemLayer`/`TransportFacade`/`Entropy`, verified OS-agnostic by the spine deep-read):

- `lifecycle.rs` — the whole fail-closed reducer (states/events/effects, lifecycle.rs:16-125)
- `broker.rs` — submit/on_grant/on_clause/poll_once incl. M6/M8/M9/M16 and
  `POLL_LOOKBACK_SECS` (broker.rs:28-39)
- `enactor.rs` — trait + registry + `EnactContext` (fail-closed `NoEnactor`, enactor.rs:71)
- `error.rs`, `audit.rs`, `ports.rs` (EventSink + `TimeLeftState::Unknown`),
  `transport_facade.rs` (+ MockTransport)
- `multi_child.rs` — per-child isolation, ExtensionInbox (multi_child.rs:32-42, 128-267)
- `child_policy.rs` — Signet-first precedence (child_policy.rs:62-115)
- `enforcer_runtime.rs` — tick/ledger/eod logic, `lock_message` copy
  (enforcer_runtime.rs:241-254); the Linux-only `managed_freeze_target` and
  `apply_effects` stay behind `#[cfg]` or move back into charterd
- `lock_info.rs` — the child-facing "when can I come back" copy in clause tz
  (lock_info.rs:30-106); pure over charter-schedule, needed verbatim by the Android lock UI
- `curator_sync.rs`, `enforce_mode.rs`, `status_emit.rs`, `device_code.rs`
- `enactors/time_extend.rs` — reused near-verbatim on Android (time_extend.rs:25-124)

Stays in charterd: `runtime.rs` (the Linux loop host), `dbus_service.rs`, `dbus_surface.rs`,
`enactors/install_flatpak.rs`, `enactors/exec_allow.rs`, `exec_guard.rs`, `device_limits.rs`,
`managed_guard.rs`, `pairing_setup.rs`, `web_content.rs`, `bin/`.

H2 acceptance: charterd depends on charter-spine; total test count across both workspaces
equals the pre-H2 baseline; `linux/` five gates green.

### 1.4 Workspace topology — two sibling workspaces (decided)

```
core/
  Cargo.toml            # workspace: members = crates/*; [workspace.package] +
                        #   [workspace.dependencies] subset copied verbatim from linux/Cargo.toml:43-79
  Cargo.lock            # new, committed
  rust-toolchain.toml   # channel 1.94.1 + targets = ["aarch64-linux-android","x86_64-linux-android"]
  rustfmt.toml          # copy (edition 2021, max_width 100)
  deny.toml             # copy (linux/deny.toml:3-16)
  crates/<the 9 + charter-spine>
linux/                  # unchanged workspace minus the moved members
android/
  jni/                  # standalone crate ([workspace] empty table), path-deps ../../core/crates/*,
                        #   own committed Cargo.lock
  app/                  # Kotlin/Gradle (§3)
```

Not one root workspace, because sibling workspaces keep `linux/Cargo.lock` byte-stable
(path-dep moves are invisible to the lock), keep `linux/ci/linux-ci.yml`'s
`working-directory: linux` and the `.deb` packaging untouched, and keep android-only deps out
of the Linux lock (hoist-analysis §5; matches concept.md:232-234).

`linux/` repoints: delete the moved entries from `members`/`default-members`
(`linux/Cargo.toml:3-19, 25-40` — **preserve the deliberate omission of charter-lock from
`default-members`**); rewrite internal path deps (`linux/Cargo.toml:68-79`) to
`{ path = "../core/crates/charter-…" }` preserving the existing `default-features = false`
flags. Consumers needing repoint: charterd (7 core deps + testkit dev-dep), charter-cli
(charter-primitives), charter-webpolicy (charter-content + testkit). charter-ipc touches none.

While touching: normalize charter-content/charter-webpolicy to workspace-inherited
`version`/`license` (`charter-content/Cargo.toml:3-4`).

**Feature-discipline footgun:** every shared crate defaults to `mock`. `android/jni` must
depend with `default-features = false` everywhere, and the Android CI gate must assert the
shipped `.so` was built without `mock` (the analog of `linux/ci/linux-ci.yml:38-39`).

**Two guards move with the hoist:**
1. The birthdate privacy guard is hard-pinned to scan `linux/crates` only
   (`charter-sys/tests/privacy_birthdate_guard.rs:85-92`) — the hoist silently removes the
   moved crates from its coverage. Relocate it to charter-testkit (or duplicate) with scan
   roots covering `core/crates`, `linux/crates`, **and `android/`** (Kotlin included: extend
   the pattern set to `.kt`). This lands in the same PR as H1 or the guard is a lie.
2. A lock-parity CI step: every package common to `core/Cargo.lock` and `linux/Cargo.lock`
   must resolve to the same version (the two workspaces can now skew).

### 1.5 Verification checklist (run in order; H1 ships only when all green)

1. Baseline: record test counts from `cargo test --workspace` and
   `cargo test --workspace --features real` in `linux/` pre-hoist.
2. H1 = pure `git mv` + Cargo.toml edits + the Ioctl fix; `git diff -M --stat` shows 100%
   renames for all `.rs` and `vectors/` files.
3. Post-hoist, run all five gates (`linux/ci/linux-ci.yml:27-39`: fmt, clippy
   `--all-features -D warnings`, mock test, real test, `--no-default-features --features
   real` build) in **both** `linux/` and `core/`; tests(linux′) + tests(core′) == baseline.
4. `diff` old vs new `linux/Cargo.lock` — expect no version changes.
5. Lock-parity script green (§1.4 guard 2).
6. Golden suites (`nip44_vectors`, `nip59_giftwrap`, `interop_nostr_tools`) pass in `core/`
   — proves the `CARGO_MANIFEST_DIR` vector resolution survived the move.
7. Android cross-check gate: `cargo ndk -t arm64-v8a -t x86_64 check` for all moved crates
   `--no-default-features`, plus `charter-sys --no-default-features --features real-relay`.
8. CI wiring: `linux/ci/linux-ci.yml` is **not yet in `.github/workflows/`** — when wiring,
   add a `core/**` path-filtered job running the same five gates with
   `working-directory: core`. The moved crates' tests leave the linux gate, so the core gate
   ships in the same PR — mandatory, not optional.

---

## 2. `android/jni` — the Rust `.so` surface

### 2.1 Crate shape

`android/jni/` — crate `charter-jni`, `crate-type = ["cdylib"]`, lib name `charter_jni`.
Deps (`default-features = false` on all): charter-spine, charter-verify, charter-schedule,
charter-transport, charter-proto, charter-primitives, charter-crypto,
charter-sys (`features = ["real-relay"]`), plus `jni`, `tokio` (rt, current_thread only),
`getrandom`, `libc`. A `mock` feature (off by default) gates the test-only entry points
(§2.5) and pulls charter-testkit for instrumented interop tests in debug builds.

### 2.2 Android impls of the carried ports

- **Persistence (all 8 ports)** — reuse the Linux `Real*Store` impls unchanged
  (`charter-sys/src/persistence.rs:396-478`): pure filesystem, atomic temp+fsync+rename
  (`charter-sys/src/fsutil.rs:34-46`), already tested headlessly under `--features real`.
  Only the base dir changes: the hardcoded `/var/lib/charter` (`persistence.rs:429`) is
  injected via `charterInit(baseDir)` using the existing `with_base` constructors. Base dir =
  the app's **device-protected storage** files dir (Direct Boot: enforcement must run before
  first unlock — decision D6). Keep the subject_hex path-traversal sanitiser
  (`persistence.rs:624-633`). Single-process invariant: the in-process Mutex serialisation
  (`persistence.rs:408-414`) requires **all Charter components in one `android:process`** —
  never set a custom `android:process` on any component; add a lint/test for it.
- **Clock** — new `AndroidClock`: `now_utc()` from `std::time::SystemTime` (wall, may jump);
  `monotonic_millis()` from `libc::clock_gettime(CLOCK_BOOTTIME)` — **not** `std::time::
  Instant`/CLOCK_MONOTONIC, which stops during suspend (`charter-sys/src/clock.rs:88-90`
  gotcha); a phone dozes constantly and budget accounting must count across it.
- **MachineSigner** — `RealMachineSigner::load_or_create` unchanged
  (`charter-sys/src/signer.rs:125-150`), path re-rooted to `<baseDir>/machine.key`. This key
  IS the device identity (concept.md:65-71). Fail-safe semantics carry: missing/corrupt key
  → zeroed pubkey, signing errors, degrades closed (signer.rs:82-94). The same scalar is the
  NIP-44 ECDH key (signer.rs:133-137). Android Keystore cannot hold secp256k1/BIP-340 keys —
  the raw-scalar-in-file model carries over; the app sandbox + FBE replaces 0600-root.
  **`android:allowBackup="false"`, no autoBackup rules, key never leaves the device** — a
  factory reset destroying it is the §3 no-port property, do not undermine it with backup.
- **Entropy** — `getrandom` (bionic `getrandom(2)`); keep the Linux semantics: panic rather
  than proceed with a predictable nonce (`charterd/src/runtime.rs:85-98`).
- **RelayTransport** — `RealRelayTransport` carries verbatim (rustls + compiled-in
  webpki-roots, no OS cert store; connection-per-op, 8s timeout, `relay.rs:268-347`).
  Scheduling lives above the port in Kotlin (§3.7). Wrapped in an
  **`AndroidTransportFacade` decorator** adding a durable outbound spool: on
  `publish_request`/`emit_audit`/status publish where **all** relays report `Failed`, spool
  the already-built wrap JSON to `<baseDir>/outbox/`, retry on each slow poll, drop after
  7 days (mobile offline windows are routine, not exceptional — Linux's accepted-loss
  fire-and-forget at runtime.rs:139-141 is not acceptable on a phone). STATUS spools
  latest-only per subject; REQUEST/AUDIT queue in order.
- **GuardianSigner / SeedSigner** — test seams only, `mock`-gated; never a production
  guardian signer on the device (signer.rs:1-3,17-23).

Android does **not** implement the `SystemLayer` aggregate (25 ports, mostly Linux-only,
`charter-sys/src/layer.rs:18-70`). It composes the individual ports above into a slim
`AndroidSystem` implementing only: Clock, ConsumedIds, Pending, Clauses, ChildClauses,
CuratorLists, Usage, Extension, Pairing, Machine, Relay. The Linux effect ports get
`Unsupported` stubs (they are never called: the Android loop maps decisions to Kotlin
effects, §3). The two Linux enactors that die: `ExecAllowEnactor` + `exec_guard` (deleted,
concept.md:78,85-87 — sandbox + verified boot make them moot); `InstallFlatpakEnactor`
(replaced by the install-APK bridge, §2.4). `RequestRecord.source_path` stays for wire/state
compat, always `None`.

### 2.3 Threading model

- At `charterInit`, spawn one dedicated thread **"charter-core"** running a
  `current_thread` tokio runtime. The `Broker`, `MultiChildEnforcer`, stores, and transport
  are owned by that thread — preserving the "in production a single task owns it" discipline
  (broker.rs:3-5). No `Mutex<EnforcerRuntime>` held across awaits (the tick is synchronous
  by design, enforcer_runtime.rs:124-164).
- Every JNI entry point marshals its args, sends a command over an mpsc channel to
  charter-core, and **blocks** on a oneshot reply with a bounded timeout (default 10s;
  `charterPollOnce` 300s — it can legitimately wait on an in-flight install, §2.4). Kotlin
  must therefore never call these from the main thread (§3.2 pins the calling threads).
- Rust→Kotlin callbacks exist for exactly **one** purpose: enact directives (§2.4). All
  other event flow is pull-based via `charterDrainEvents` — no `AttachCurrentThread`
  fan-out, no callback storms under Doze. The `EventSink` impl buffers
  `RequestUpdated`/`TimeLeftChanged`/`LockStateChanged` into a ring buffer (cap 256,
  overwrite-oldest) drained by Kotlin each fast tick. Unlike Linux — where only
  `RequestUpdated` is actually emitted and the other two are declared-but-dead
  (`dbus_service.rs:114-118`, zero emit sites) — Android implements **all three for real**;
  the lock activity and notifications consume them.
- Timekeeping: `AndroidClock` is authoritative inside Rust. JNI calls that evaluate time
  (`charterTick`, `charterPollOnce`) also take `nowUnix` from Kotlin and log if it diverges
  from `AndroidClock::now_utc()` by >5s (tamper/skew telemetry); the Rust clock wins.
  Elapsed charging is computed **in Rust** from the measured delta, clamped to
  `4 × poll_interval` (the runtime.rs:810-829 rule) — Kotlin never computes elapsed.

### 2.4 The enact bridge (install.apk)

`TimeExtendEnactor` runs entirely in Rust (registered verbatim). Install-APK needs
`PackageInstaller`, which is Kotlin-side. To keep **`Effect::Enact` producible only from a
verified Allow enforced in Rust** (lifecycle.rs:1-3,64-65), the bridge is an `Enactor` impl
in charter-jni, not a Kotlin decision path:

1. `KotlinBridgeEnactor::enact(grant, ctx)` (running on charter-core, inside
   `Broker::on_grant`'s M8 retry loop, broker.rs:230-243) re-validates the **signed grant
   params** (never request params), then invokes the registered JVM callback
   `CharterCallbacks.onEnactApk(reqId, paramsJson)` via a pre-attached JNIEnv, and awaits a
   oneshot with a **180s timeout**.
2. Kotlin (§3.5) starts the PackageInstaller session **asynchronously** and returns from
   the callback immediately. The session result arrives on a BroadcastReceiver and calls
   `charterEnactResult(reqId, status, detail)` — which resolves the oneshot.
3. `status` maps to the Linux error taxonomy: `"ok"` → `EnactOutcome`; `"transient"` (IO,
   installer busy, network fetch failure) → `EnactError::Transient` (broker retries the
   same in-memory `VerifiedGrant`, never re-verifies/re-consumes — M8); `"terminal"`
   (signing-cert mismatch = the `Conflict` analog, malformed package) →
   `EnactError::Terminal`. Timeout → `Transient`.
4. No deadlock: the blocked JNI caller is the slow-tick worker thread (§3.2), never the
   thread the installer result arrives on.

Idempotence and the TOCTOU-close analog live Kotlin-side but are contract-pinned (§3.5):
already-installed-at-same-or-newer-version+same-signer = no-op success (the
`InstallFlatpakEnactor` skeleton, install_flatpak.rs:56-71); the fetched APK's signing-cert
digest must equal the signed grant's `signerCertSha256` **before session commit**, mismatch
= terminal, nothing installed (replaces `admit`'s copy-then-rehash,
effects.rs:1201-1226; concept.md:116-118).

### 2.5 The JNI function inventory

All functions are `external fun` on `org.forgesworn.charter.native.CharterNative`; all
payloads are UTF-8 JSON strings; hex is **strict lowercase** (the serde layer rejects
uppercase, `charter-primitives/src/ids.rs:49-61` — Kotlin must never emit uppercase hex).
Errors surface as a thrown `CharterException(code, message)` where `code` is the stable
string form of `SysError`/`BrokerError`/`VerifyError`/`IpcError` — keeping `Conflict`
(refuse, nothing stored) distinguishable from `Io` (transient, retry), which the retry and
fail-closed behavior depend on (`charter-sys/src/error.rs:9-23`).

**Lifecycle / identity / pairing**

| Function | Signature | Rust wiring |
|---|---|---|
| `charterAbiVersion` | `() -> Int` | compile-time constant; Kotlin refuses to run on mismatch |
| `charterInit` | `(baseDir: String, enforceMode: String, callbacks: CharterCallbacks, nowUnix: Long) -> String` | builds `AndroidSystem` (stores `with_base(baseDir)`), `RealMachineSigner::load_or_create`, `AndroidTransportFacade`, `Broker::new` (runs the **M9 Enacting→Failed reconcile**, broker.rs:66-79), registers `TimeExtendEnactor` + `KotlinBridgeEnactor`, restores usage/extension snapshots. Returns `{machinePubkey, paired, guardianShort, reconciledCount, enforceMode}` |
| `charterShutdown` | `() -> Unit` | flush snapshots, drop runtime |
| `charterDeviceCode` | `() -> String` | machine pubkey grouped 8×8 (`charterd/src/device_code.rs:47-53`) |
| `charterPair` | `(bunkerUri: String) -> String` | `pin_from_connect` (`charter-transport/src/pairing.rs:34-85`) **with the percent-decode fix (§5.1)**; persists `pairing.json`; pin-once — re-pair requires the parent-gated setup flow (§3.8), mirroring real.rs:303-307's refusal |
| `charterPairingState` | `() -> String` | `{paired, guardianShort}` — 8-char fingerprint, never the full key (dto.rs:100-105) |

**Child/UI surface (the bound service's backing calls — contract.rs:38-51 semantics)**

| Function | Signature | Rust wiring |
|---|---|---|
| `charterSubmitRequest` | `(op: String, paramsJson: String) -> String` | `Broker::submit` (broker.rs:127-171; **M16 persist-Pending-before-publish** inside). Ops: `time.extend`, `install.apk` (§5.4). Unpaired → `NotPaired` |
| `charterQueryStatus` | `(reqId: String) -> String` | `Broker::status` (broker.rs:367); `""` = all |
| `charterListRequests` | `(limit: Int) -> String` | `Broker::list`, newest-first (broker.rs:379) |
| `charterCancelRequest` | `(reqId: String) -> Boolean` | `Broker::cancel` — true iff was Pending (broker.rs:388-401) |
| `charterTimeLeft` | `(subjectHex: String?) -> String` | `MultiChildEnforcer::remaining` → `TimeLeftView` mapping (dbus_service.rs:82-94); null subject = sole ward; unknown/missing snapshot → **`TimeLeftView::unknown()` = locked, never unlimited** (dto.rs:84-95) |
| `charterLockInfo` | `(subjectHex: String?) -> String` | `lock_info` + `lock_message`: reason title, "come back at HH:MM today/tomorrow/Mon" in **clause tz**, grouped weekly hours, used-today line (lock_info.rs:30-106, enforcer_runtime.rs:241-254). The lock activity renders this verbatim — no Kotlin tz/DST math, ever |
| `charterDrainEvents` | `() -> String` | drains the `DaemonEvent` ring buffer (§2.3) |

**Loop surface (driven by the Kotlin service, §3.3)**

| Function | Signature | Rust wiring |
|---|---|---|
| `charterTick` | `(activeSubjectHex: String?, screenInteractive: Boolean, nowUnix: Long) -> String` | per-tick: reload cached clauses from the store (**every tick** — this is how guardian edits apply, enforcer_runtime.rs:94-121; never cache across ticks), `resolve_effective` per ward (child_policy.rs:62-115), drain `ExtensionInbox` (multi_child.rs:32-42), compute clamped elapsed in Rust, `MultiChildEnforcer::tick` (only the active ward accrues; `screenInteractive=false` ⇒ nobody accrues), persist usage+extension snapshots, return `[ChildDecision]` JSON: `{subject, active, locked, reason, effects:[showLock{reason,title,detail}|hideLock|freeze|thaw|warn{ten|one}|audit{kind}], remaining}`. `reason` is **level state**, not the edge (multi_child.rs:107-124) |
| `charterPollOnce` | `(nowUnix: Long) -> String` | `Broker::poll_once` (clauses → curator lists → grants, broker.rs:340-358; cursor = `now − POLL_LOOKBACK_SECS` 2 days, broker.rs:28-39), flush outbound spool, throttled STATUS build/publish (state-change OR 60s heartbeat, status_emit.rs:60-76), `purge_expired(now)` on the consumed-id store (**Linux never calls it — the store grows unboundedly; do not inherit that leak**, persistence.rs:22). Returns `{grantsProcessed, clausesAccepted, publishFailures, statusEmitted}` |
| `charterEnactResult` | `(reqId: String, status: String, detail: String) -> Unit` | resolves the enact-bridge oneshot (§2.4) |
| `charterSetEnforceMode` | `(mode: String) -> Unit` | `EnforceMode::{Observe,FreezeOnly,Enforce}`; unrecognized/absent → **Enforce** (fail-closed default, enforce_mode.rs:17-46). "FreezeOnly" reads as "suspend-only, no LockTask" on Android |

**Test-only (feature `mock`, debug builds; drives the golden-vector interop suite on-device)**

- `charterTestVerifyGrant(eventJson, reqIdHex, nonceHex, nowUnix) -> String` — error codes
  mirror `VerifyError` verbatim (verify.rs:76-99) for parity assertions.
- `charterTestEventId(eventJson) -> String` / `charterTestVerifySignature(eventJson) -> Boolean`
  — NIP-01 canonical id + BIP-340 checks against `golden/nostr/*.json`.
- `charterTestInjectClause(eventJson, nowUnix) -> String` — the full broker clause path
  (parse → `highest_issued_at` → `verify_clause` → `put_child_clause`, broker.rs:279-333,
  incl. absent-subject → sole-child routing) without a relay.
- `charterTestForceClock(nowUnix)` — MockClock swap for deterministic instrumented tests.

**What is deliberately NOT on this surface:** any enact/approve/install/grant method. The
frozen no-local-authority allowlist (contract.rs:67-77, dbus_surface.rs:42-49) ports as a
unit test over the JNI symbol table and the bound-service API (§6.2).

### 2.6 `.so` packaging

- Build: `cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs build --release
  -p charter-jni` (release adds `--no-default-features --features real-relay`; debug adds
  `mock` for the test entry points). Toolchain pinned by `core/rust-toolchain.toml` +
  `android/jni/rust-toolchain.toml` (1.94.1, both targets).
- ABIs: **arm64-v8a** (Pixels/GrapheneOS) + **x86_64** (charter-ci emulator). No armv7/x86.
- 16 KB page alignment: NDK r28+ default; **assert in CI** with
  `llvm-readelf -l libcharter_jni.so | grep LOAD` (align 0x4000) — Android 15/GrapheneOS
  requirement.
- CI asserts the release `.so` graph contains no `mock`: `cargo tree -e features -p
  charter-jni --no-default-features --features real-relay | grep -c mock == 0`.
- tzdb (chrono-tz) and TLS roots (webpki-roots) are compiled in — schedule evaluation is
  bit-identical to Linux regardless of Android tzdata; root-store updates ship only via app
  update (accepted; note in release checklist).

---

## 3. `android/app` — the Kotlin enforce half

### 3.1 Module layout

Single Gradle module `android/app` (package `org.forgesworn.charter`), Kotlin, packages:

```
admin/     CharterDeviceAdminReceiver, ProvisioningActivity, provisioning helpers
service/   CharterService (foreground), BootReceiver, PackageChangeReceiver, TickScheduler
enforce/   AppGateOps, LockScreenOps, RestrictionOps, UsageSource  (interfaces + Dpm* impls + Fake* impls)
enact/     ApkInstallOps (PackageInstaller bridge), ApkFetcher
bind/      CharterLocalService (bound service), DaemonEvent Flow
native/    CharterNative (JNI decls), CharterCore (typed wrapper), CharterCallbacks
ui/        LockActivity, OnboardingActivity, StatusActivity, AdminSettingsActivity
```

Each `enforce/` interface has two impls — a **fake** (in-memory, call-recording,
failure-injecting: `installCalls()`, `setFailTimes(n)`, `isSuspended(pkg)`,
`lastLockMessage()`, mirroring the Rust mock ergonomics) for JVM/Robolectric tests, and a
**Dpm-backed real** exercised only in instrumented tests + production. This is the
mock/real discipline carried to Kotlin (§6.4).

### 3.2 Threads

- `charter-core` (Rust-owned, §2.3).
- `CharterWorker` — a single `HandlerThread`; owns the fast tick and all fast-path JNI
  calls (`charterTick`, `charterDrainEvents`, `charterTimeLeft`).
- `CharterSlowWorker` — a second single-thread dispatcher; owns `charterPollOnce` (may
  block up to the enact timeout) and outbound retries. **One wedged binder or network call
  costs a tick, never the enforcer** — the two-thread split is the Android analog of the
  hard-timeouted `loginctl` lesson (runtime.rs:255-269; hardening #14). A watchdog alarm
  re-posts the tick if a tick overruns 3× its cadence.
- Main thread: UI only. No JNI call from main, enforced by a debug-build strict check.

### 3.3 The loop host (CharterService)

Foreground service (type `specialUse`/`dataSync`), started by `BootReceiver`
(`BOOT_COMPLETED` + `MY_PACKAGE_REPLACED`) and by the DPC on provisioning. Replaces
`runtime.rs::run` with the same shape:

- **Fast tick — 2s while the screen is interactive, event-driven otherwise.** Register for
  `SCREEN_ON`/`USER_PRESENT` and re-tick immediately on wake, so a curfew lock lands within
  ~1s of screen-on (hardening #13) — never rely on a timer surviving Doze. While screen-off,
  fall back to a 60s alarm tick (usage can't accrue with the screen off; the clamp makes the
  gap harmless).
- **Slow tick — every `pollInterval` (default 10s foreground, 60s screen-off; D4)**:
  `charterPollOnce`, event drain for notifications, snapshot of publish failures.
- Per fast tick: `UsageSource` resolves foreground package + screen state →
  `charterTick(activeSubject, screenInteractive, now)` → apply each `ChildDecision`:
  - `locked` → `AppGateOps.reconcile(locked=true)` + `LockScreenOps.show(title, detail)`
  - unlocked → `reconcile(false)` + `LockScreenOps.hide()`
  - `warn{ten|one}` → high-priority notification with the exact Linux copy
    ("Charter — time's almost up" / "10 minutes left" / "1 minute left — save your game",
    runtime.rs:350-382)
  - `audit{kind}` → nothing local; the Rust side already queued the AUDIT wrap.
- **Order preserved: ShowLock strictly before suspend** (enforcer.rs:305-312) — the lock
  surface must be up before the apps freeze.
- **Startup self-heal, exact ordering** (the `thaw_all` analog, runtime.rs:419-428,
  786-794, inverted for Android where suspension **persists** across reboot — the failure
  direction flips from "lost the freeze" to "stuck suspended"): on every service start,
  first reconcile suspensions/LockTask to *freshly computed* evaluator state (clear anything
  current policy doesn't demand), then resume enforcement. Never trust remembered state.
- Any "pause enforcement" admin affordance is **volatile** — process-memory only, cleared
  by reboot (the `/run/charter/paused` property, runtime.rs:396-405; hardening #19).

### 3.4 The DPC (admin/)

`CharterDeviceAdminReceiver : DeviceAdminReceiver` — minimal; `onEnabled` starts
`CharterService`. Manifest: `BIND_DEVICE_ADMIN`, device-admin XML declaring no legacy
policies (DO doesn't need them). Debug builds set `android:testOnly="true"` so
`dpm remove-active-admin` works during development; release builds do not.

`RestrictionOps` (the moral successor of the Phase-9 posture probes
Mounts/Polkit/Account/Systemctl):
- **Baseline set, applied only once a charter exists (invariant I17):**
  `DISALLOW_INSTALL_APPS` + `DISALLOW_INSTALL_UNKNOWN_SOURCES` (the load-bearing
  install-lockdown, concept.md:89-102), `DISALLOW_CONFIG_DATE_TIME` + auto-time required
  (the `timedate1` analog closing the clock-tamper hole the ledger deliberately doesn't
  defend, usage.rs:85-88; hardening #5), `DISALLOW_FACTORY_RESET`, `DISALLOW_ADD_USER`.
- **`DISALLOW_SAFE_BOOT` + `DISALLOW_DEBUGGING_FEATURES` — SHIPPED 2026-08-07 (S1).**
  These remove the OS's own escape routes, so they waited on ours existing first
  (hardening #20, I27, D9). Break-glass (jni/breakglass.rs + LockActivity) is that
  hatch and has shipped, so the deferral had nothing left to wait for. Safe mode
  disables every third-party package, a Device Owner included: no tick, no lock, no
  suspension, and — because nothing runs — no report either. Belt to the brace:
  STATUS now carries `enforcementGap`, a count of boots the warden did NOT run
  through (contract.md §STATUS), so any other route to an unwarded boot still
  surfaces to the guardian.
- `posture() -> Report`: re-read every restriction each slow tick, re-assert drift,
  emit an audit tag on drift (level-triggered, like everything else).

### 3.5 Enactors mapped from Linux

**1. Install-APK (replaces `InstallFlatpakEnactor`, install_flatpak.rs:25-81).**
Kotlin half of the §2.4 bridge. Contract, in order:
1. Parse/validate the **signed grant params** (`{packageName, versionCode?,
   signerCertSha256, source}`, §5.4) — the identity re-parse analog of
   `FlatpakRef::parse`-before-any-OS-call (install_flatpak.rs:56-57).
2. Idempotence: `PackageManager.getPackageInfo(packageName)` in the ward user — installed
   at ≥ versionCode with matching signer → no-op success (install_flatpak.rs:62-71).
3. Fetch the APK (`ApkFetcher`: parent-staged local file in v1; URL source deferred, D7).
4. **Signing continuity before commit**: compute the APK's signing-cert SHA-256
   (`PackageManager.GET_SIGNING_CERTIFICATES` on the archive) and require equality with the
   grant's `signerCertSha256`; mismatch → terminal `Conflict`, session abandoned, nothing
   installed (the TOCTOU-close/`admit` analog, effects.rs:1201-1226; concept.md:116-118).
5. Commit a DO `PackageInstaller` session (silent — DO is exempt from its own lockdown,
   concept.md:132-135), targeting the ward user.
6. Installer/network failures → `"transient"` (the TrustDb `set_fail_times` retry
   semantics, effects.rs:369-378, now exercised through M8's 3-attempt loop).

**2. App-gate via suspend (replaces `CgroupFreezer`, effects.rs:174-184).**
`AppGateOps.reconcile(locked)` — **level-triggered every tick** (reconcile_freeze,
enforcer.rs:44-66; hardening #1):
- Package set = **live enumeration** of installed launchable packages in the ward user,
  never a curated stale list (the whole-surface lesson, hardening #18).
- Deny-list guard (the `is_valid_freeze_target` analog, enforcer.rs:28-34): never suspend
  own package, the lock UI, dialer/emergency, Settings, the default launcher, system
  input/keyboard. The platform exempts some of these — assert anyway.
- **Check the return value**: `setPackagesSuspended` returns the packages it FAILED to
  suspend — treat as `exists()==false` (no-op, retry next tick), log every one; never an
  error, never swallowed (effects.rs:179-183 contract; hardening #18).
- Re-assert on `BOOT_COMPLETED`, `ACTION_PACKAGE_ADDED/REPLACED`, profile start — Android
  suspension persists, so the reconcile must both apply *and clear*.
- `setApplicationHidden` reserved for hard-hide of store packages (T5 fallback, spike).

**3. Time-extend** — pure Rust, verbatim (`enactors/time_extend.rs`): u16 ≤1440 validation,
0-minute no-op success, **`exp ≤ clause-tz EOD + 300s`** where EOD = later of schedule-tz
and budget-tz midnights (time_extend.rs:79-90, enforcer_runtime.rs:203-234; never
`TimeZone.getDefault()` — hardening #8), dimension from echoed `limitHit`, idempotent by
reqId, deposited into the `ExtensionInbox` drained by the live enforcer (hardening #11:
exactly one enforcing ledger; the Kotlin side never keeps a parallel display model).

### 3.6 Budget loop input (`UsageSource`) and the lock UX

`UsageSource` (promoted to a port — on Linux this was inline `loginctl`,
runtime.rs:200-207):
- Foreground app via `UsageStatsManager.queryEvents` (event-timestamped
  `ACTIVITY_RESUMED`/`PAUSED` spans — prefer events over polling, hardening #12), screen
  state via `PowerManager.isInteractive` + display listener.
- Maps to `Activity`: `Active` = ward foreground + screen interactive; suspended-by-Charter
  time = `FrozenByCharter` = **credits zero** (usage.rs:13-27) — never burn quota while
  gated.
- DO self-grants `PACKAGE_USAGE_STATS` (`setPermissionGrantState`); spike **T6** proves
  readability on the target build — if unreadable, that is the genuine blocker the spike
  gate exists for.
- If the usage query fails, **preserve the prior enforcement state and skip one tick's
  accrual** — never collapse to "nobody managed" (the fail-closed roster read,
  runtime.rs:834-849).

**LockActivity (replaces charter-lock; LockTask replaces override-redirect + grabs +
overlay + VT lock).** Humane-lockout semantics preserved exactly
(docs/superpowers/specs/2026-07-03-humane-lockout-design.md:28-41, 119-130):
- Content = `charterLockInfo` verbatim: reason-specific title ("Outside allowed hours" /
  "Time's up for today" / "Locked — Setup needs attention, ask your guardian"), the
  "You can come back at…" line in clause tz, grouped weekly hours ("Mon–Fri 07:00 – 20:00"),
  "up to 2h a day — 1h 05m used today". Reason passed as **current level state in the
  intent extras on every show**, never a remembered edge (hardening #13).
- Warnings 10 min + 1 min, once each, re-armed after thaw / rising edge / unbounded spell
  (enforcer.rs:322-342) — free, `EnforcerCore` ports verbatim. **No at-zero grace** — at 0
  the lock shows (approved decision; do not soften silently).
- `setLockTaskPackages([own])` + `startLockTask` on show; `stopLockTask` + finish on hide.
  Show is idempotent while shown (effects.rs:1405-1409); show/hide are best-effort from the
  tick, re-asserted level-triggered.
- **Never trapped**: sanctioned actions always on screen — "Ask for more time" (paired
  only, **one-shot per lock**, fixed 30-minute opening ask, immediate "Asked! Your guardian
  will see it shortly." confirmation, charter-lock/main.rs:71-73, 503-525), power off, and
  Android's mandatory emergency-call affordance. The ask composes `time.extend` with **M7
  limitHit routing**: authoritative `reason` → sole-zero heuristic (`schedule==0 &&
  budget!=0` → schedule) → default budget (dispatch.rs:107-137) — a bedtime lock that asks
  with `limitHit:"budget"` lands minutes in the wrong pool and never unlocks.
- **Never suspend anything the lock experience depends on** (the frozen-compositor lesson,
  hardening #17); LockTask is OS-composited so the class is structurally moot, but emit a
  "lock actually landed" audit signal (activity `onResume` under lock task mode) — on-metal
  rounds are where these bugs live.
- Escape hatch (pre-GA blocker, D9): a **parent-PIN-gated pause** on the lock screen
  (volatile, reboot-cleared) and/or a guardian-remote pause clause — never a learnable
  chord (charter-lock/main.rs:128-130 flags the Linux chord as alpha-only).

### 3.7 Bound service + relay IO under Doze

**`CharterLocalService`** (in-process, `exported=false`, signature permission) replaces
`org.forgesworn.Charter1` 1:1 (concept.md:61-62):
- Methods exactly: `submitRequest(op, paramsJson)`, `queryStatus(reqId)`,
  `listRequests(limit)`, `cancelRequest(reqId)`, `timeLeft()` — same unpaired semantics
  (friendly `NotPaired` message, `[]`, `false`; dbus_service.rs:153-219). Plus read-only
  `pairingState()`. **No enact/approve/grant method — pinned by test** (§6.2).
- Events: a `SharedFlow<DaemonEvent>` fed from `charterDrainEvents` — all three events live
  (`RequestUpdated` with the fresh full status row on every lifecycle transition,
  `TimeLeftChanged`, `LockStateChanged`).
- Caller identity: `Binder.getCallingUid()` — never an intent extra (hardening #24). In v1
  single-ward this collapses to "the DO app's own UI"; keep the keying so a future
  multi-ward device shows each ward only their own numbers.
- Error taxonomy `NotPaired/NotFound/Denied/Offline/Invalid` carried across the boundary —
  it is the UX vocabulary (charter-ipc/src/error.rs:8-25).

**Relay IO under Doze:** the DO exempts its own package from battery optimization and runs
the foreground service persistently (spike **T10** proves the heartbeat survives doze).
v1 keeps the connection-per-poll `RealRelayTransport` (it tolerates wifi↔cellular churn by
construction) at 10s foreground / 60s screen-off cadence; the 2-day cursor lookback
(broker.rs:39) makes any missed window self-healing, and reprocessing is idempotent
(issuedAt floor + consumed ids). A persistent websocket with live REQ subscription +
ConnectivityManager-driven backoff is deferred (D4). The 60s STATUS heartbeat is the
guardian's wipe-detection signal (concept.md:164-170); Doze will produce legitimate
multi-minute gaps → the guardian-side gap detector gets a mobile-tolerant threshold (D5),
decided explicitly, not by accident.

### 3.8 Provisioning, identity, re-enrollment

**ADB provisioning onboarding** (the known rough edge, concept.md:54-58 — QR/NFC/zero-touch
are dead on GrapheneOS because Play is sandboxed):
1. Factory-fresh device, **no accounts**. Parent installs the Charter APK (adb install).
2. `OnboardingActivity` walks the parent through: enable developer options → USB debugging
   → run (from the repo helper `android/scripts/charter-provision.sh`):
   `adb shell dpm set-device-owner org.forgesworn.charter/.admin.CharterDeviceAdminReceiver`
   (spike **T0** — must succeed once; everything depends on it).
3. App detects `isDeviceOwnerApp` → `charterInit` mints the machine identity
   (load_or_create) → shows the **device code** (8×8 grouped hex; the PWA parser strips all
   whitespace so the display round-trips, deviceCode.ts:8-24; hardening #29) → parent enters
   it in MyCharter, and pastes MyCharter's `bunker://` URI into the app → `charterPair`.
4. Until a charter (any time clause) exists, the device is **inert**: no restrictions, no
   suspensions, no lock (invariant I17). Staged bring-up via `charterSetEnforceMode`:
   observe → suspend-only → enforce, default enforce (hardening #22) — this structures
   decented's hardware gates.
5. A brick guard: provisioning refuses to finish in a state with no owner-side admin path
   (the charter-setup analog, linux/packaging/setup/charter-setup:44-55).

**Device identity + no-port re-enrollment** (concept.md:65-71, 241-244): the machine key is
minted at first init, lives only in device-protected app-private storage, is excluded from
every backup path, and is destroyed by factory reset. Post-reset: new key ⇒ old wraps
undecryptable, old pairing addresses a dead key ⇒ re-enrollment shows a **new** device code
and the guardian signs the fresh install **as a new device**. Stale clauses replayed at the
new install die on the per-(subject,kind) monotonic `issuedAt` floor
(persistence.rs:68-85); stale grants die on pending-state + consumed ids. The recovery-wipe
escape stays detect-not-prevent: the tell is the guardian-side STATUS heartbeat going dark
(§3.7, §5.3).

**User topology (D1, decided default):** v1 targets **ward-on-primary-user** — the phone is
the kid's device, user 0, DO enforces in place, the parent is remote via the PWA. The
concept's "kid = secondary profile, parent = Owner" shared-device model (concept.md:59-60)
requires the affiliated-PO topology (DO in user 0 + same package as profile owner of the
secondary user + `setAffiliationIds` for LockTask to work there) — deferred, and the spike's
T3 result informs it. `MultiChildEnforcer` keys by subject either way, so the core doesn't
care.

---

## 4. The inherit-list — hardened invariants and how Android honors them

Numbered; each names the pinned Linux source and the Android mechanism. These are the port
review's checklist: for every enforcement surface ask *"who reads this, and what happens
when the apply fails?"* — the single most repeated historical failure class is **silent
fail-open**, the second is **edge-vs-level**, the third is **wrong target under
multi-user** (hardening cross-cutting notes).

**I1 — Enact only from a verified Allow.** `Effect::Enact` is producible solely from
`(Pending, GrantVerified(Allow))` (lifecycle.rs:1-3, 78-83); `VerifiedGrant` has no public
constructor (verify.rs:39-46). Android: the reducer ships unmodified in charter-spine; the
JNI boundary erodes the type-level proof, so compensate structurally — the **only** source
of enactable params is the enact-bridge callback issued from inside `Broker::on_grant`;
Kotlin has no code path that constructs an "approved" object from request data (§2.4).

**I2 — Forged/unauthenticated grant stays Pending (M6).** `GrantRejected` emits nothing and
is non-terminal (lifecycle.rs:93-99); only an authenticated Deny or TTL expiry is terminal
— hostile-relay denial-of-approval stays closed (HANDOFF:125-129). Android: verbatim via
spine. Do not "improve" this.

**I3 — Single-use, atomic, durable-before-return.** `verify_grant` consumes the reqId
**before** returning (verify.rs:160-171); retention window `exp + 300` matches the
freshness window (M4, verify.rs:15); store error → fail-closed `VerifyError::Store`;
`check_and_consume` is atomic (persistence.rs:14-23) and durable via Mutex +
temp+fsync+rename (persistence.rs:406-478). Android: same Rust store over app-private
files; the fsync-before-return property is why the persistence impls port unchanged rather
than being rewritten on SharedPreferences. Deny also consumes (a later allow for the same
reqId is `Replayed`).

**I4 — Retry same VerifiedGrant, never re-verify/re-consume (M8).** ≤3 attempts on
transient enact errors on the in-memory grant (broker.rs:230-243). Android: the enact
bridge maps `"transient"`/timeout into this loop (§2.4).

**I5 — Crash mid-enact reconciles to Failed (M9).** Reloaded `Enacting` → `Failed`
"interrupted before completion; please re-request" at `Broker::new` (broker.rs:66-79) —
the consumed grant is unrecoverable, enactors are idempotent, re-request is safe. Android:
runs on every `charterInit`, i.e. every service (re)start.

**I6 — Persist Pending before publish (M16).** A lost Pending record silently drops the
guardian's grant (broker.rs:157-164). Android: verbatim; plus the outbound spool (§2.2)
retries the publish itself.

**I7 — Monotonic `issuedAt` is the only clause rollback defense; never now-gates.**
Enforced twice: `verify_clause` (clause.rs:88-93, equal counts as rollback) and the store
write (persistence.rs:38-46, 68-85), per-(subject,kind). Now-based clause gates were tried
and reverted (M5/M15 — a dead RTC must not block a guardian *tightening*;
HANDOFF:114-129). Android: verbatim; the platform half of the clock defense is
`DISALLOW_CONFIG_DATE_TIME` + auto-time (§3.4), and usage accounting uses CLOCK_BOOTTIME
(§2.2).

**I8 — Wrap freshness bounded both directions at both endpoints; cursor lags 2 days.**
Device: `|now − wrap.created_at| ≤ MAX_JITTER_SECS = 2d` in `unwrap_with_author`
(nip59.rs:19, 171-222). Guardian: `MAX_WRAP_JITTER_SECS` mirror (request.ts:27, commit
99d16e5). Poll cursor = `now − 2d` so relay `since` filters never permanently exclude a
backdated wrap (broker.rs:28-39); safe because reprocessing is idempotent. Android:
verbatim via transport + spine; §5.6 pins cross-stack constant parity so neither side
drifts.

**I9 — Never fake-approve a device ask offline (guardian side).** A wire-correlated
decision with no signer THROWS; the ask stays pending with a visible reason
(decisionGate.ts:15-27, commit 99d16e5). Android's design assumption: an answer may never
arrive — keep the pending lifecycle, surface "asked — waiting" honestly on the lock screen,
and never infer approval from anything but a verified GRANT.

**I10 — Unknown is locked, never unlimited.** `TimeLeftView::unknown()` = locked, reason
"unknown", offline (dto.rs:84-95); `TimeLeftState::Unknown` ≠ unlimited (ports.rs:42-48).
Android: `charterTimeLeft` returns `unknown()` for missing snapshot/unresolvable caller.

**I11 — Malformed schedule locks and extensions never mask it; budget fail-open asymmetry
is deliberate.** Authenticated-but-unparseable schedule → synthetic paused → lock
(enforcer_runtime.rs:100-111); malformed forces schedule=0 regardless of extension (M1,
enforcer.rs:227-237); unparseable budget → dimension unconstrained (child_policy.rs:16-19)
— schedule remains the gate. Android: replicate deliberately (it ships in core); do not
"fix" silently.

**I12 — Level-triggered reconcile; edges are lossy.** `reconcile_freeze` every tick
(enforcer.rs:44-66; hardening #1). Android inversion: suspension **persists** across
reboot, so reconcile must both apply and clear; re-assert on boot/package-change/profile
events; check `setPackagesSuspended`'s failed-package return (§3.5).

**I13 — Exactly one enforcing ledger; status is a projection.** The
extension-updates-readout-only bug (hardening #11) is why: enacted extensions go through
`ExtensionInbox` into the ledger the tick reads (multi_child.rs:514 test). Android: Rust
core owns the one evaluator state; Kotlin enforcement and the bound service both read it;
no parallel Kotlin display model.

**I14 — Charge measured elapsed, clamped `0..=4×interval`.** Clock steps, doze gaps,
suspend-resume never credit as a burst (runtime.rs:810-829). Android: computed in Rust
(§2.3); monotonic source = CLOCK_BOOTTIME; `FrozenByCharter` credits zero; a forward jump
past midnight resets by design, restart cannot refill quota (usage.rs snapshot semantics;
corrupt snapshot silently restores fresh — accepted, noted).

**I15 — Warnings 10/1, once each, exact re-arm rules; no at-zero grace.** Re-arm after
thaw, on rising edge, and across an unbounded spell (enforcer.rs:313-342; spec decisions
1-2). Android: verbatim via `EnforcerCore`; delivery = high-priority notifications (§3.3).

**I16 — Never trapped; never bricks the shared device; pause never durable.** Sanctioned
exits always on screen; per-ward enforcement; the pause flag is volatile
(runtime.rs:396-405). Android: §3.6 lock UX; §3.3 volatile pause; emergency call always
reachable.

**I17 — Fail-closed applies only to configured surfaces; a fresh install is inert.** The
unconditional web force-lock bug (web_content.rs:129; hardening #21). Android: no
restriction/suspension/lock before a signed charter exists for the ward; each surface gates
on its own is_configured (§3.4, §3.8).

**I18 — Audit carries classification only; child content never rides it.** AUDIT content
is always empty; tags = `[["outcome",…],["op",…]]`, vocabulary
`enacted|denied|failed|locked|thawed` (audit.rs:16-48, contract.md:257-264). Child `reason`
rides only the E2E wrap, never audit/logs (params.rs:62-66). Plus the repo-wide no-DOB
guard (§1.4). Android: verbatim; keep the strings (`locked/thawed` map cleanly to
suspend/unsuspend).

**I19 — STATUS heartbeat is the wipe-detection signal.** Emit on displayable state change
OR 60s heartbeat (status_emit.rs:60-76); its **absence** is the guardian-side audit-gap
tell (concept.md:164-170). Android: emitted from `charterPollOnce` on the slow cadence;
Doze-tolerant guardian threshold is D5; spike T10 proves feasibility.

**I20 — No enact/approve/grant method on any user surface.** Frozen allowlist test
(contract.rs:67-77). Android: same test over the bound-service API + JNI exports (§6.2).

**I21 — Caller identity is unspoofable.** Linux: bus credentials (dbus_service.rs:165-177).
Android: `Binder.getCallingUid()`, never an intent extra (hardening #24).

**I22 — Unreadable identity/usage input preserves prior enforcement state.** Fail-closed
roster read (runtime.rs:834-849). Android: a failed UsageStats/user query skips one tick's
accrual and keeps the current lock state (§3.6).

**I23 — Never enforce against the enforcement surface itself.** `is_valid_freeze_target`
analog: deny-list guard on suspend targets (§3.5); never suspend/hide the DO app, lock UI,
dialer, Settings-critical packages (hardening #17, #18).

**I24 — All-blocked is explicit wire `paused:true`; empty weekly = always allowed.** The
inverted-semantics fail-open (clause.ts:56; hardening #26). Android: inherits via wire +
core evaluator; this spec documents it so no Kotlin surface re-derives it wrong. Related:
per-date override REPLACES the weekly entry — empty override array = fully blocked day
(schedule_eval.rs:120-126); no midnight-crossing windows (`start < end`, clause.rs:38-42).

**I25 — Multi-dimension saves share one `issuedAt`** (store.tsx:676; hardening #27) —
guardian-side, but any future device-local clause emitter follows the same rule; the
monotonic floor is what makes a stale re-sign dangerous.

**I26 — Local settings writes merge, never clobber** (device_limits.rs:760; hardening
#25). Android: the admin UI does read-modify-write on ward config through one serialized
writer; `subject` binding and local limits stay independent fields.

**I27 — The warden must never strand its own guardian.** Escape hatch before
`DISALLOW_SAFE_BOOT`; provisioning brick guard (hardening #20; §3.4, §3.8, D9).
SATISFIED 2026-08-07: break-glass shipped first, then the restriction. The
ordering is the invariant, not the restriction — anything that removes an OS
escape route must land AFTER ours, never alongside it.

**I28 — Install-lockdown is capability-closure, not channel-enumeration.**
`DISALLOW_INSTALL_APPS` + unknown-sources closes every store at once (concept.md:89-102);
the only door back in is the guardian-signed install enacted by the DO (spike T1/T2). If
one channel slips (spike T1 fallback), belt-and-suspenders `setApplicationHidden` on that
store's package.

---

## 5. PWA / wire-contract deltas

Baseline verdict (guardian-half deep-read): **the wire is platform-generic** — machine =
pubkey, no OS field on any payload; an Android warden implementing the device side of
`spec/contract.md` (pin from bunker URI, signed-inner verification, per-(subject,kind)
issuedAt HWM, ±300s grant skew, EOD cap, marker tag `["t","charter-device"]`) pairs with
and is managed by the deployed PWA **today** for schedule/budget/time.extend. The deltas:

**5.1 Fix the bunker-URI percent-encoding mismatch (pre-existing latent bug — fix before
any Android pairing).** `guardianBunkerUri` emits `relay=wss%3A%2F%2F…`
(guardianPairing.ts:11 `encodeURIComponent`); `pin_from_connect` requires a literal
`wss://` prefix and rejects the whole URI as `MissingRelays` (pairing.rs:57-62); the CLI
validator has the same gap (charter-cli/src/pairing.rs:39-44). Live pairing has never been
exercised on metal, so it has not been caught. Fix **both** sides: device/CLI
percent-decode relay values (repairs already-deployed PWAs), and the PWA stops encoding
plain `wss://` URLs. Add a round-trip test: PWA-emitted URI → `pin_from_connect` → Ok.

**5.2 Device kind (cosmetic, one session).** `Device.platform` is the literal union
`"linux"` (domain/types.ts:87) and `addDevice` hardcodes it (store.tsx:423): add
`"android"`, a device-kind choice + phone copy/emoji/default-label in Family
(Family.tsx:177-181, 237, 285-296). While there, fix two pre-existing pairing bugs an
Android field pairing will trip: the Reconnect/"rescan" path clobbers a real devicePubkey
with a `mockpub_…` (Family.tsx:263, store.tsx:439-454), and an unparseable code silently
falls back to a fake key (store.tsx:445-446) — fail loudly instead. No wire field for
platform is added (STATUS field set is still open [decide], contract.md:310-312; the label
is PWA-local).

**5.3 Wire STATUS into the guardian UI.** `subscribeStatus`/`liveStatusFor` are built and
tested but not imported by any screen (wire/status.ts:100-121, store/liveStatus.ts:20-51);
device AUDIT (31000) is not consumed at all. Once the Android warden emits STATUS, wire
the store + a "last seen" indicator — this is the §7 audit-gap surface ("wiped phone goes
dark"). Gap threshold: D5.

**5.4 New op `install.apk` (additive contract change — never mutate the frozen op).**
`install.flatpak` is flatpak-locked at the type level (`Remote` enum permits only
`flathub`, fail-closed, params.rs:13-26) and golden-vector-pinned. Add:
- charter-proto: `OpType::InstallApk` = wire `"install.apk"`; request params
  `{packageName, label?, source}`; grant params `{packageName, versionCode?,
  signerCertSha256, source}` — parse fail-closed per-op like flatpak (grant.rs:18-46).
  `signerCertSha256` is required on the **grant** (the guardian pins the provenance; the
  device enforces continuity, §3.5).
- PWA: extend `RequestOp` (wire/types.ts:67), un-drop at parse (request.ts:29-32, 70 —
  today only `time.extend` survives), type `GrantPayload.params` as a union
  (wire/types.ts:108), add grant building (grant.ts is time.extend-only). Approvals cards
  already render install asks with the correct single-use semantics ("this app, this once",
  Approvals.tsx:63-72).
- Golden vectors: new `golden/nostr` + `golden/proto` fixtures for install.apk
  request/grant, signed with nostr-tools, run on both stacks.
- Phasing: land the contract + vectors with the port (cheap, additive); Kotlin enactment
  ships in Phase 2 (D2). `exec.allow` is Linux-only and simply never emitted by an Android
  device (op stays frozen in the contract for the Linux line).

**5.5 Per-app clauses — the one genuinely new contract surface — deferred (D3).** No wire
carrier exists for (ward, app) gating: `ClauseKind` is schedule|budget|content
(wire/types.ts:50, contract.md:219), app-scope policies emit zero clauses
(wire/clause.ts:94-99) — app blocking is UI-local fiction today. MVP enforcement is
device-level (whole-ward suspend on schedule/budget). When built: new `ClauseKind`
`"app"` with `{appId, blocked?, schedule?, budget?}` + `policyToClauses` emission +
device-side verify/enact + per-(subject,kind,appId) rollback keying. `appId` strings are
already opaque end-to-end in the PWA, so Android package names flow unchanged.

**5.6 Cross-stack constant parity, pinned in the contract.** Add a `spec/contract.md`
table + parity tests on all three stacks (Rust core, PWA, Kotlin config): freshness skew
**300s**; wrap jitter **2 days** (nip59.rs:19 ↔ request.ts:27); `MAX_EXTEND_MINUTES`
**1440**; `MAX_REASON_LEN` **280**; kinds **1059/13/31111/31112/31113/31114/31000**
(kinds.rs:5-26); warn thresholds **600/60s**; next-open lookahead **8 days**; marker tag
`["t","charter-device"]`; STATUS heartbeat **60s**; lowercase hex everywhere.

**5.7 Unchanged and load-bearing (Android replicates exactly):** signed-inner GRANT/CLAUSE
with sig preserved through the wrap vs UNSIGNED REQUEST rumor + machine-signed seal +
`rumor.pubkey == seal.pubkey == payload.machine` (nip59.rs:21-25, request.ts:104-134);
reqId/nonce/limitHit echoed verbatim; deny = signed 0-minute grant (treat inbound 0-minute
as an answered deny, not an extension); grant wrapped to exactly one machine; minutes u16
bounds; reason privacy; grouped device-code display convention.

---

## 6. Test plan

### 6.1 Rust core (host)

- The ~390 existing tests move with their crates; §1.5 pins count parity. Five gates run in
  both workspaces; `core/` gates protect both substrates (concept.md:232-234).
- charter-jni host tests (feature `mock`): broker/enforcer wiring through the command
  channel, enact-bridge state machine (ok/transient/terminal/timeout → M8), event ring
  buffer, outbound spool retry/expiry, error-code mapping table (every
  `VerifyError`/`SysError` variant has a stable JNI code — exhaustive match, compile-broken
  by new variants).
- Golden vectors extended: **next-day schedule opens** (the ae67427 gap — TS/Rust parity
  never covered them; hardening #7), install.apk request/grant vectors (§5.4), a
  PWA-emitted bunker URI fixture through `pin_from_connect` (§5.1).
- Cross-compile gate: `cargo ndk -t arm64-v8a -t x86_64 build -p charter-jni
  --no-default-features --features real-relay` + 16 KB alignment assert + no-mock assert
  (§2.6).

### 6.2 Kotlin unit tests (JVM/Robolectric — the mock gate)

Against the `Fake*` port impls (§3.1):
- Reconcile loop: level-triggered suspend apply/clear; failed-package return handling
  (inject failures, assert retry next tick, assert logged); deny-list guard (attempt to
  suspend own package → refused); boot/package-change re-assert.
- Lock flow: ShowLock-before-suspend ordering; reason-as-level-state in intent extras;
  one-shot ask button state machine; M7 limitHit routing table (reason → sole-zero
  heuristic → budget default, all four cases from dispatch.rs:107-137).
- UsageSource mapping: foreground+interactive = Active; suspended = FrozenByCharter;
  query failure = skip-accrual + preserve state.
- Enact bridge Kotlin half: idempotence short-circuit, signer-digest mismatch → terminal +
  session abandoned, fetch failure → transient.
- **Surface-freeze test (I20):** reflection over `CharterLocalService`'s API + the
  `CharterNative` externals asserting no method name contains
  enact/approve/install*/grant (allowlisting `charterEnactResult` as the completion
  callback and the request-op constant), mirroring contract.rs:67-77.
- Single-process lint: manifest parse test asserting no component sets `android:process`.

### 6.3 Emulator e2e (charter-ci AVD, `-gpu off`) — mapped to the spike contract

Instrumented tests with the app provisioned as DO on the emulator
(`dpm set-device-owner` in the test harness; `testOnly` debug build):

| Spike | Instrumented test |
|---|---|
| **T1 install lockdown** | after a charter is configured, assert `DISALLOW_INSTALL_APPS` + unknown-sources active; attempt a non-DO `PackageInstaller` session + `pm install` → both fail. Also assert the **inert-before-configured** inverse (I17) |
| **T2 silent install** | inject a signed install.apk grant via `charterTestInjectClause`-style test transport → enact bridge → test APK installs with no UI; wrong `signerCertSha256` variant → terminal, not installed |
| **T4 suspend gate** | drive a budget clause to exhaustion with `charterTestForceClock` → tick → test app suspended (launch intercepted); inject a verified time.extend → unsuspended within one tick; kill+restart the service mid-locked → state reconciled, not lost |
| **T6 usage signal** | self-grant usage access; foreground the test app; assert `UsageSource` spans feed the ledger and `charterTimeLeft` decrements |
| **T9 reboot persistence** | reboot the emulator (or at minimum force-stop + boot-receiver restart): DO intact, restrictions intact, startup self-heal reconciles suspensions from recomputed state, machine key + pairing + consumed-ids + issuedAt floors all survive |

Golden-vector interop runs on-device through the test-only JNI entry points (§2.5) —
the nostr-tools-signed fixtures are the cross-implementation truth.

**What the emulator cannot prove** (LockTask/DO semantics differ, GrapheneOS restriction
honoring is undocumented): T0 provisioning on the target build, T7 LockTask feel, T8
hardening set, T10 doze heartbeat — these are decented's on-metal spike/staged-bring-up
gates (observe → suspend-only → enforce), exactly like the Mint-VM tail on Linux.

### 6.4 Mock/real discipline on Android

Five-gate parity, per substrate:

| Linux gate | Android analog |
|---|---|
| `cargo fmt --check` | ktlint/detekt + `cargo fmt` (jni) |
| clippy all-features `-D warnings` (mock+real co-compile) | same for charter-jni (mock + real-relay together) |
| `cargo test --workspace` (mock) | Gradle JVM/Robolectric unit tests vs fakes |
| `cargo test --features real` (headless real cores) | `connectedAndroidTest` on charter-ci AVD (real DPM) |
| `--no-default-features --features real` build proof | `assembleRelease` + no-mock `.so` assert + 16 KB align assert |

Pin AGP/NDK/cargo-ndk versions (hardening #30 — CI must run what production runs). The
final gate is always on-metal GrapheneOS with decented, staged via enforce-mode.

### 6.5 End-to-end device↔guardian

- Headless: instrumented test with a `TestGuardian` (charter-verify `test_support`) over
  `MockTransport` — full submit → grant → verify → enact → unlock loop on-device without
  a relay.
- Live: one scripted round against `wss://relay.trotters.cc` with the deployed PWA
  (after §5.1 lands): pair, sign a schedule, watch the lock land, ask-for-more from the
  lock screen, approve on the phone, watch the unlock — decented's hardware gate, turnkey
  steps prepared in advance.

---

## 7. Open decisions deferred (with defaults — the build never blocks)

| # | Decision | Recommended default (build proceeds on this) |
|---|---|---|
| **D1** | User topology: ward-on-primary vs kid-as-secondary-profile (concept.md:59-60) | **Ward-on-primary-user (user 0), single ward, parent fully remote.** Affiliated-PO shared-device topology deferred; core is subject-keyed either way (§3.8). Spike T3 informs the revisit |
| **D2** | install.apk phasing | **Contract + vectors + PWA types now (§5.4); Kotlin enactment Phase 2.** MVP ops on-device: time.extend (+ inert install.apk submit path behind a flag) |
| **D3** | Per-app clauses (the (ward, app) tuple, concept.md:123-126) | **Defer.** MVP = device-level schedule/budget over the whole ward surface. Wire design sketched in §5.5; needs its own golden-vector round |
| **D4** | Relay transport: poll vs persistent websocket | **Keep connection-per-poll** at 10s foreground / 60s screen-off; 2-day lookback self-heals gaps. Persistent socket + ConnectivityManager backoff only if battery/latency data demands it |
| **D5** | Guardian-side heartbeat gap threshold under Doze | **"Live" indicator at 3×60s missed; audit-gap alert at 6h** of use-time silence. Tune after T10 data; decide explicitly, never by accident (transport reader item 4) |
| **D6** | Storage: device-protected vs credential-encrypted | **Device-protected (Direct Boot) for all Charter state incl. machine.key** — enforcement must run before first unlock; FBE device-key encryption + app sandbox is the 0600 analog. Revisit only if threat model changes |
| **D7** | Update engine scope (concept.md §6 dial) | **Curated-store-alive:** Accrescent/F-Droid self-update in the ward profile stays possible via guardian-approved installs; the controller's fetch-and-push engine covers only Play-only stragglers, and v1 ships with **parent-staged APK files** (no URL fetcher) |
| **D8** | Sandboxed Play Services in ward profile (FCM push) | **Default none (fully Google-free).** Per-family opt-in later; informational spike T11 |
| **D9** | The "warden wedged" escape hatch (pre-GA blocker) | **Parent-PIN-gated volatile pause on the lock screen** + guardian-remote pause clause; `DISALLOW_SAFE_BOOT`/`DISALLOW_DEBUGGING_FEATURES` were **not applied until this shipped** (I27). **CLOSED 2026-08-07** — break-glass shipped, both restrictions are now in the baseline, and `enforcementGap` reports any unwarded boot that gets through anyway |
| **D10** | minSdk / targetSdk | **minSdk 34, targetSdk 35** (GrapheneOS tracks current Android; 16 KB pages assumed). Lower minSdk only if a real /e/OS / Calyx target demands it |
| **D11** | Pairing UX: QR vs typed code | **Typed/pasted 8×8 code first** (the parser contract exists both ways, hardening #29); QR scan later — note the PWA viewfinder is currently decorative (Family.tsx:367-397) |
| **D12** | Audit-kind reconciliation with Signet's guardian-authored feed (contract.md:260-264) + `audit_transparency` dual-wrap + STATUS field set (contract.md:310-312) | **Inherit the open [decide]s unchanged** — Android emits exactly what Linux emits (31000 machine-authored, empty content; 31114 per §5.6). Resolve wire-side once, both wardens pick it up |
| **D13** | Multi-device usage consolidation (phone + laptop per kid) | **Nothing now** — `StatusPayload.machine` is already the aggregation key and USAGE_SYNC 31115 is reserved + contract-complete (contract.md:314-393); no wire change needed when it comes |
| **D14** | Kotlin-side republish of Pending requests older than N with no relay ack | **Covered by the transport-level outbound spool (§2.2)** — no broker change; revisit only if spool data shows request loss |

---

### Build sequence recap (spike-gated, per concept.md:198-199)

1. **Spike on metal** (spike.md T0–T11 hard gate) — no Charter code.
2. **H1 hoist** (§1.1-1.5) + privacy-guard move + CI wiring — one PR, zero-behavior-change
   proof attached.
3. **H2 charter-spine extraction** (§1.3) — second PR, test-count parity.
4. **§5.1 bunker-URI fix + §5.4 contract additions + golden vectors** — small PR, unblocks
   both live pairing and the Android op.
5. **android/jni** (§2) — host-tested against mock, cross-compiled, interop vectors green.
6. **android/app** (§3) — JVM tests → emulator e2e (§6.3) → decented's staged on-metal
   rounds (observe → suspend-only → enforce).
