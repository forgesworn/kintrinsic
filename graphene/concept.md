# Charter for GrapheneOS — concept & feasibility notes

> Captured from an early conceptual conversation (2026-06-28, extended 2026-06-30).
> **Not a spec.** These
> are honest feasibility notes meant to preserve the thinking, not to direct a build.
> Opinionated where it helps; explicit about the limits.

## 1. The framing: this is not a port of `charterd`

`charterd` is a **privileged Linux daemon** that brokers root-level syscalls —
flatpak, fapolicyd, cgroups, X11/Wayland input grabs, polkit, systemd, noexec mounts.
GrapheneOS has **none** of those primitives and forbids root and privileged daemons
outright. So "port charterd to the phone" is the wrong mental model — roughly half of
charterd is the wrong half for Android.

The right model: a **GrapheneOS Device Owner app** that **reuses Charter's protocol
core** (the request → guardian-signs → verify → enact → audit loop, the crypto, the
schedule/budget brain) and **re-expresses enforcement** through Android's
`DevicePolicyManager`.

A pleasant surprise falls out of this: a lot of charterd's *hardest* engineering — the
fapolicyd trust-DB, the TOCTOU-safe approved-exec store, noexec mounts, the
confused-deputy exec guard — exists only to **retrofit execution control onto Linux**.
**Android provides that structurally** (app sandbox + verified boot + package manager).
So moving to the phone *deletes* work as well as adding it.

## 2. What carries over vs what gets rebuilt

**Reuse — the "decide" half (OS-agnostic):**

- `charter-schedule` — the schedule/budget evaluator (pure logic, the screen-time brain)
- `charter-verify` — the 6 grant rules + signed-clause auth + monotonic-`issuedAt`
  rollback protection
- `charter-crypto` (BIP-340), `charter-proto`, `charter-transport` (NIP-44/59
  gift-wrap, `bunker://` pairing, audit)
- `charterd`'s **spine** — the fail-closed lifecycle reducer, the single-use-atomic
  broker, the enactor registry, the enforcer runtime
- The **guardian half** is already shared with the Linux line: Signet / the Charter
  PWA, the same wire contract. Minimal new work.

**Rebuild — the "enforce" half (Android Device Owner):** the entire `charter-sys`
Linux port layer is thrown away and re-expressed via `DevicePolicyManager`. And it's
**smaller** than the Linux version, because the OS hands you sandboxing + verified boot.

**Decided — Rust-via-JNI with a shared core (was the deferred open question):** keep the
Rust core and cross-compile it into the Android app via JNI/NDK, *not* a Kotlin
reimplementation — a Kotlin rewrite would fork the verification logic and break the "one
crypto backend / one evaluator" invariant. Kotlin owns only the Android-side enforcement
(`DevicePolicyManager`); Rust owns decide/verify/crypto, single-sourced with the Linux
warden. Where that code physically lives is laid out in §9.

## 3. The Device Owner architecture

- **Max privilege on GrapheneOS = Device Owner (DO).** No root. Provisioned **once via
  `adb shell dpm set-device-owner`** at first setup. QR / NFC / zero-touch enrollment
  does **not** work, because GrapheneOS runs Google Play sandboxed/unprivileged — so
  the enrollment paths that route through privileged Play are dead. (Confirmed by the
  GrapheneOS founder.)
- **Kid = secondary user profile; parent = Owner.** Per-profile encryption keys; the
  Owner can already toggle off a secondary profile's app-install ability from Settings.
- The Charter spine runs **inside** the DO app; the Linux D-Bus surface becomes an
  in-process bound service.
- The guardian still approves by **signing the same Nostr events**. The wire contract,
  crypto, and approval UX are unchanged. That is the strategic payoff.
- **Enrollment binds the charter to *this* device, and a reset breaks the binding.**
  Grants are bound to a device identity minted at first setup; a factory reset destroys
  it. Coming back under Charter therefore means the guardian **signs a fresh device as
  new** — the old charter and grants cannot be migrated or replayed onto the reset
  install. This is what makes the recovery-wipe escape safe (see §7), and it dovetails
  with `charter-verify`'s monotonic-`issuedAt` rollback protection: a stale charter
  pointing at a dead device identity is refused, not honored.

### Port mapping (highlights)

| Linux (charterd) | GrapheneOS equivalent | Verdict |
|---|---|---|
| flatpak install | `PackageInstaller` (DO silent install) + `DISALLOW_INSTALL_APPS` | rebuild, easier |
| fapolicyd trust-DB / approved-exec store / noexec / exec-guard | — Android sandbox + verified boot | **delete, moot** |
| cgroup freeze | `setPackagesSuspended` / `setApplicationHidden` | rebuild, cleaner |
| X11/Wayland undismissable lock | **LockTask mode** (`setLockTaskPackages` + `startLockTask`) | rebuild |
| VT switching / mounts / polkit / systemd / admin groups | — no analog needed | **delete / replaced by the model** |
| schedule/budget evaluator | reuse `charter-schedule` verbatim | **reuse** |
| relay IO / clock / persistence / signer | carry over | **reuse** |

The three Linux enactors (`install_flatpak`, `exec_allow`, `time_extend`) collapse to
essentially **"gate an app"** and **"grant/extend time"** — `exec_allow` and its whole
supporting cast vanish.

## 4. The install-lockdown insight (the load-bearing idea)

Don't enumerate and filter the app channels — there are too many (Play, sideloaded APK,
F-Droid, Aurora, **Accrescent**, the GrapheneOS App Store "Apps"). Instead, as Device
Owner, **close the install capability itself**: `DISALLOW_INSTALL_APPS` +
unknown-sources restriction. **Every channel goes dark at once.** The store count stops
mattering.

Then the **only** door back in is the parent: the parent approves an app, and the
controller **silently installs the APK** via `PackageInstaller`. That is *exactly*
Charter's existing loop — request → guardian signs → enact — so the `install_flatpak`
enactor simply becomes an install-APK enactor. The model maps on almost suspiciously
well.

## 5. The Duolingo / Play-only straggler corner

The kid's ideal is **no app store and ideally no Play apps** — but in practice one
real app (here: **Duolingo**) is closed-source and Play-distributed. This is the one
genuinely messy corner, and it resolves cleanly:

- **"No store for the kid" and "Duolingo present" do not conflict.** The constraint
  that matters is *"the kid can't install things,"* not *"no closed-source app may
  exist on the device."* Duolingo is installed **once**, by the parent (sourced via
  Aurora in the Owner profile, or APKMirror), pushed into the kid's profile. Install
  capability stays globally off afterward. The Play Store app doesn't even need
  removing — with installs blocked at the OS level it's an inert icon.
- **Integrity reassurance:** Android refuses any update not signed with the original
  app's key. So you only need to trust the **first** APK's provenance; a swapped or
  malicious mirror APK can't install as an update. This makes auto-update from a mirror
  reasonably safe.
- **Push notifications nuance:** Duolingo's streak reminders ride Google's FCM, which
  needs sandboxed Google Play **Services** (separate from the Play **Store**) in the
  kid profile. Optional. Without it, Duolingo runs on an email/password account, just
  no push (which some parents call a feature).
- This lands on Charter's grain: a Charter is one *(kid, app)* tuple with its own
  schedule/budget. "Duolingo, this kid, 30 min/day, 4–7pm" is a single signed grant.
  The curated-single-app shape is the model working as intended, not a workaround.

## 6. Updates don't have to be manual

The worry: closed-source apps update constantly, so are we stuck hand-pushing APKs
forever? **No — but updates must all funnel through the controller.**

- A **Device Owner can install/update any app silently, no prompt, and is exempt from
  its own install lockdown** (it installs programmatically via `PackageInstaller`,
  which isn't a "user install"). So the controller can update Duolingo at 3am and
  nobody touches anything.
- So it's **automated if you build the update engine, hand-cranked if you don't.** The
  controller becomes the kid's store + update daemon: poll sources on a schedule, fetch
  new APKs, check signing-key continuity, silent-install.

**The work-reducing split:**

- **Open-source apps (F-Droid / Accrescent):** these stores are *built* for seamless
  unprivileged updates on modern GrapheneOS (Accrescent especially). Largely let them
  self-update; don't reinvent it.
- **Play-only stragglers (Duolingo & friends):** no clean auto-updater on GrapheneOS,
  so *these* are what the controller's fetch-and-push engine is for. A much shorter
  list.

**The tradeoff dial:** blunt lockdown (controller updates *everything* → you're the
device's whole update engine) vs. curated-store-alive (Accrescent/F-Droid self-update,
controller only babysits the Play-only stragglers). Most would pick the second.

**The number that decides how much work this is:** how many Play-only apps does the kid
realistically need? A short list (Duolingo + 2–3) → a small, bounded controller update
engine → the comfortable end. Fifteen closed-source Play apps → reconsider.

## 7. The hard limits & the threat model

- **No Factory Reset Protection on GrapheneOS** → recovery-mode wipe always escapes all
  controls; `DISALLOW_FACTORY_RESET` only blocks the in-Settings path. This is **not the
  threat** in a trust-aligned system — it's the accepted escape, the exact analog of a kid
  booting **Tails from USB** past the Linux warden. Two properties make it safe to design
  *around* rather than *against*:
  - **The audit gap is the tell.** Charter logs screen time continuously, so a wiped phone
    still in use shows up as *use with no time logged*. Detection is **parent-side** — a
    wiped phone reports nothing, so the signal is the guardian watching the audit stream go
    dark while the device is plainly still in the kid's hand (an absent heartbeat, like a
    laptop that stops checking in), not an alarm the device raises about itself.
  - **A reset can't carry the charter forward.** Re-enrollment mints a **new device**: the
    guardian signs the fresh install as new and the old charter/grants cannot be migrated or
    replayed onto it (see §3). The recovery-wipe escape costs the kid the device's whole
    charter and routes the way back through the parent.

  **Design detect-not-prevent**, consistent with Charter's declared **cooperative /
  anti-casual** model — nothing changes philosophically.
- To stop *casual* bypass: lock the bootloader, leave OEM-unlocking off, set
  `DISALLOW_SAFE_BOOT` + `DISALLOW_DEBUGGING_FEATURES` (DO-only).
- **No built-in screen-time / parental API** on GrapheneOS, and **Family Link doesn't
  work** — which is fine, since you bring your own scheduler (`charter-schedule`).
- **Don't build on Shizuku** — it's reboot-fragile (loses its privileges every reboot
  without root). Device Owner is the stable foundation.

## 8. Before committing anything: the spike to run

No GrapheneOS doc enumerates exactly which `DevicePolicyManager` restrictions are
honored on a given build. So the first real-hardware step (whenever this stops being
conceptual) is: provision **TestDPC or OwnDroid as Device Owner** on the target
GrapheneOS device and confirm, on-device, that `set-device-owner`, package suspension,
LockTask, and the specific restriction set all behave. The ADB-provisioning onboarding
UX is the known rough edge. Prove feasibility on metal before investing.

This spike is written up as a fillable Go/No-Go checklist in [`spike.md`](spike.md) —
the exact capabilities to test (T0–T11), expected results, and the hard-gate pass/fail
criteria — runnable with no Charter code, just the off-the-shelf DPC stand-in.

## 9. Where it gets built — the repo layout

Decided structure — scaffolded only *after* the spike is green. Build order:
**spike → hoist `core/` → `android/jni` → `android/app`.**

**Named by substrate, not by OS.** The Device Owner app targets the whole de-Googled
Android family (GrapheneOS, /e/OS, CalyxOS, LineageOS — `DevicePolicyManager` is AOSP-wide),
so the code lives in a new top-level **`android/`**, sibling to `linux/`. `graphene/` stays
the docs + spike + "prove-it-here" home: it is the OS we *prove on*, not the substrate we
*build for*. Same monorepo — keeps the shared core adjacent and wire-contract changes atomic.

**Prerequisite — hoist the OS-agnostic core out of `linux/`.** The decide + guardian crates
(`charter-schedule`, `charter-verify`, `charter-crypto`, `charter-proto`, `charter-transport`,
`charter-primitives`, `charter-content`) currently sit under `linux/crates/`. Building Android
against them would mean depending on a path called "linux," which is backwards. Step zero is
lifting them into a shared workspace at the repo root; the Linux-specific crates
(`charter-sys`, `charterd`, `charter-lock`, `charter-ipc`, `charter-cli`, `xtask`) stay under
`linux/`.

```
core/                 # hoisted shared Rust crates — schedule, verify, crypto, proto, transport, …
linux/                # charterd — the Linux enforce half (unchanged)
android/
  jni/                # Rust cdylib: wraps core, exposes verify/schedule/crypto over JNI;
                      #   built with cargo-ndk → .so for arm64-v8a (+ x86_64 for the emulator)
  app/                # Kotlin/Gradle — the enforce half itself:
                      #   DeviceAdminReceiver/DPC, DISALLOW_INSTALL_APPS, PackageInstaller
                      #   enactor, setPackagesSuspended + UsageStatsManager budget loop,
                      #   LockTask, the in-process bound service replacing charterd's D-Bus
                      #   surface, the ADB-provisioning onboarding
```

The split mirrors `charterd` exactly: **Kotlin owns Android-side enforcement, Rust owns the
decide/verify/crypto brain** — single source of truth, reached over JNI instead of
in-process.

**Build toolchain:** Gradle (app) + cargo-ndk (the Rust `.so`), with its own CI gate (Gradle
assemble + instrumented test). The hoisted `core/` keeps running the existing Rust
fmt/clippy/test gates — which now protect *both* substrate targets at once.

---

### Open threads to revisit

- Blunt-lockdown vs curated-store-alive (how much update engine to build).
- Whether the kid profile carries sandboxed Play Services (for FCM/push) or goes fully
  Google-free.
- The re-enrollment-after-wipe protocol — minting a new device identity and refusing to
  migrate or replay the old charter onto the reset install (the no-port rule from §3), plus
  how the guardian-side audit-gap signal is surfaced.
