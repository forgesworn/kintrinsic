# Ultracode kickoff brief — Charter on GrapheneOS

> The turnkey plan for when the trigger is given: **"ultracode GrapheneOS — build it
> all."** Read this first, confirm the gates, then run. This is a planning artifact; the
> build is gated on the prerequisites below. Companion to [`concept.md`](concept.md) and
> [`spike.md`](spike.md).

## The trigger & what it means

"Ultracode" = a full multi-agent **Workflow** that builds the *entire* GrapheneOS Charter,
not a sketch. It **begins with a deep review of the hardened Linux implementation** — the
bugs and edge cases already solved under `linux/` must be inherited by the port, not
rediscovered — then builds the Android side end-to-end and verifies it.

## Prerequisites — gates before kickoff (human-owned)

1. **Linux hardened.** The Linux warden works solidly, bugs resolved, tested in real use
   with the kids. This is the source of truth Phase 1 reviews.
2. **Spike green.** [`spike.md`](spike.md) run on a Pixel; all hard-gate capabilities
   (T0 provision, T1 install-lockdown, T2 silent install, T4/T5 app-gate, T6 usage,
   T9 reboot) PASS. Agents can't touch hardware — this is a human gate.
3. **Toolchain provisioned** (see setup below) — else agents write code that never compiles.
4. **Devices:** a Pixel for real test + on-metal; an Android emulator image for CI.

## Inputs the build consumes (the "everything I need")

- [`concept.md`](concept.md) — §3 Device Owner architecture, **§9 repo layout** (decided:
  top-level `android/` + hoisted `core/`, Rust-via-JNI), the port-mapping table, the
  threat model, the install-lockdown + no-port re-enrollment properties.
- [`spike.md`](spike.md) — the capability contract the enforcer must satisfy.
- **The hardened Linux implementation** under `linux/` — reviewed live in Phase 1.
- The OS-agnostic core crates (`charter-schedule`, `-verify`, `-crypto`, `-proto`,
  `-transport`, `-primitives`, `-content`) — reused via the hoist.
- The wire contract / Signet / Charter PWA guardian half — unchanged, shared.

## The workflow shape (phases)

0. **Gate check** — confirm prerequisites 1–4. Stop if any red.
1. **Deep-review the hardened Linux impl** (parallel readers): the spine (fail-closed
   reducer, single-use-atomic broker, enactor registry, enforcer runtime), `charter-sys`
   (Linux-specific vs reusable), the verify rules, the schedule evaluator, transport/relay,
   the D-Bus surface → bound service, **and the resolved bugs / edge cases** (git history +
   tests added since 2026-06-30). Output: a structured port spec — what to replicate, what
   changes for Android.
2. **Hoist `core/`** — lift the OS-agnostic crates to a shared workspace at the repo root;
   repoint `linux/`; confirm Linux still builds and all gates stay green (zero behaviour
   change). Worktree-isolated.
3. **`android/jni`** — Rust `cdylib` over `core`, JNI surface for verify/schedule/crypto/
   proto/transport; cargo-ndk → `.so` (arm64-v8a + x86_64 for the emulator).
4. **`android/app`** — Kotlin enforce half, from Phase 1's spec + concept §3/§9:
   `DeviceAdminReceiver`/DPC, `DISALLOW_INSTALL_APPS`, `PackageInstaller` enactor,
   `setPackagesSuspended` + `UsageStatsManager` budget loop, LockTask, the in-process bound
   service replacing charterd's D-Bus surface, relay IO, the ADB-provisioning onboarding.
5. **Wire + e2e verify** — request → guardian signs → verify → enact → enforce on device;
   check against spike capabilities and Linux behaviour parity.
6. **Adversarial parity + security review** — does the port preserve the fail-closed,
   single-use-atomic, monotonic-`issuedAt`, install-lockdown, and no-port re-enrollment
   invariants? Security review of the Device Owner surface. Verify before "done."

## Toolchain setup (run at kickoff if absent)

This env has **Rust only**; the Android side is entirely absent. None of this needs
secrets — plain dev tooling, installable before/at kickoff:

```
# Rust → Android targets
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk
# JDK (Kotlin + Gradle need it)
sudo apt-get install -y openjdk-21-jdk
# Android SDK + NDK via cmdline-tools, then:
#   sdkmanager "platform-tools" "platforms;android-35" "build-tools;35.0.0" "ndk;<latest>"
# Gradle: use the project wrapper (./gradlew) — no global install needed
```

## Readiness status (2026-07-06)

- ✓ Design captured — concept §3/§9, spike, port map, threat model.
- ✓ Decided — Rust-via-JNI + hoisted `core/`; `android/` named by substrate.
- ✓ Reusable core exists (under `linux/crates/`, to be hoisted).
- ✓ Linux hardened — humane lockout A+B merged; grant flow adversarially
  reviewed; ~430 real-feature tests. (Live pairing round still pending — run it
  before Phase 1 treats Linux as the source of truth.)
- ✓ **Toolchain provisioned & verified (2026-07-06):** JDK 21; SDK at
  `~/Android/Sdk` (platform-tools, `platforms;android-36`, build-tools 36.0.0,
  NDK `29.0.14206865`, emulator + `system-images;android-36;default;x86_64`);
  Rust `aarch64`/`x86_64-linux-android` targets + `cargo-ndk` (smoke `.so`
  built for BOTH ABIs); AVD **`charter-ci`** (Pixel 7, AOSP API 36) booted
  headless to `sys.boot_completed=1`, `dpm` answers. Env: source
  `~/Android/env.sh` (deliberately not in `.bashrc`). Gotchas: `/tmp` is
  noexec → give cargo installs `TMPDIR=~/.cache/cargo-tmp`; run the emulator
  with `-gpu off` headless (swiftshader crashed QEMU on this box).
- ⚠ Spike not yet run on hardware — needs a Pixel. **The one remaining gate.**

When you return and say the word, Phase 0 re-checks these, then we go.
</content>
