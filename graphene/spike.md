# GrapheneOS feasibility spike — Device Owner capability sweep

> **Status: a test plan, not a build.** This is the on-metal check to run *before*
> committing any Charter engineering to the phone. It uses an **off-the-shelf Device
> Owner stand-in** (TestDPC or OwnDroid) — **no Charter code is written or required** to
> run it. The point is to confirm, on the actual target GrapheneOS build, which
> `DevicePolicyManager` capabilities are honored, because no GrapheneOS doc enumerates
> them. Fill the **Result** column on device; the Go/No-Go gate at the bottom decides
> whether the architecture in [`concept.md`](concept.md) holds.

## Purpose & non-goals

**Proving:** that the five load-bearing Device Owner capabilities behave on real
hardware — provision, close install, silent-install as owner, gate an app (suspend),
read usage, survive reboot. If those hold, the [`concept.md`](concept.md) port maps.

**Explicitly *not* doing:** writing Charter code, wiring the relay/guardian flow,
shipping anything, or committing to a build. A failing gate here is a *cheap* "no" — that
is the whole value of running it first.

## Prerequisites

- A spare device on the **target GrapheneOS build** that can be **factory-reset freely**
  (provisioning a Device Owner requires a device with **no accounts added**).
- A computer with `adb`; USB debugging reachable for the one-time provisioning step.
- **TestDPC** (Google's sample DPC — broadest toggle coverage) *or* **OwnDroid** as the
  Device Owner stand-in. TestDPC is the easier capability-sweep tool.
- A throwaway **test APK** to use as the "parent-approved app" (any sideloadable APK).
- A second throwaway **"straggler" APK** representing a Play-only closed-source app.
- Optional: a secondary user profile to represent the kid (T3).

## The provisioning gate (run first — make-or-break)

| ID | What it proves | Action | Expected | Result |
|----|----------------|--------|----------|--------|
| **T0** | Device Owner can be set at all on this build | Factory-fresh, **no accounts**: `adb shell dpm set-device-owner com.afwsamples.testdpc/com.afwsamples.testdpc.DeviceAdminReceiver` | `Success: Device owner set...`; TestDPC reports active DO | ☐ PASS ☐ FAIL |

> If T0 fails (e.g. refuses because an account exists, or GrapheneOS rejects it), stop and
> resolve provisioning before running anything below — every other test depends on it. The
> ADB-provisioning UX being rough is *expected*; T0 only needs to *succeed once*.

## Capability tests

Each is a TestDPC/OwnDroid action (its UI exposes these as toggles); use `adb`/`dumpsys`
to observe where noted.

| ID | Capability | Action (via DPC stand-in) | Expected | Result |
|----|-----------|---------------------------|----------|--------|
| **T1** | **Install lockdown** — close every channel at once | Enable user restriction `no_install_apps` (+ unknown-sources). Then try installing from: GrapheneOS App Store, F-Droid, Aurora, Accrescent, raw-APK via file manager | **Every** channel blocked/greyed; no install completes | ☐ PASS ☐ FAIL |
| **T2** | **Silent install as owner** — the parent's door back in | With T1 still active, push the test APK via the DPC's `PackageInstaller` action | Installs **silently, no prompt** — DO is exempt from its own lockdown | ☐ PASS ☐ FAIL |
| **T3** | **Per-profile model** | Create secondary user (kid); as DO push the test APK into it; confirm install capability off there | App present in kid profile; kid can't self-install | ☐ PASS ☐ FAIL |
| **T4** | **App gate via suspend** — the budget enactor | `setPackagesSuspended(true)` on test app; launch it; then `false` | Suspended app can't launch (system dialog shown); unsuspend restores it; both responsive | ☐ PASS ☐ FAIL |
| **T5** | **App gate via hide** — alternative enactor | `setApplicationHidden(true)` then `false` | Icon gone / can't launch; reversible. (Compare UX vs T4 for the budget loop) | ☐ PASS ☐ FAIL |
| **T6** | **Usage signal** — knowing a budget is spent | Confirm `UsageStatsManager` data is readable (usage-access granted to DPC); use test app a few min; check per-app foreground time | Per-app usage readable and roughly accurate — this is the budget loop's *input* | ☐ PASS ☐ FAIL |
| **T7** | **LockTask** — the undismissable lock | `setLockTaskPackages` + start lock task (DPC's lock-task action) | Pinned single app; home/recents/nav suppressed; releases only under DO control | ☐ PASS ☐ FAIL |
| **T8** | **Anti-casual hardening** | Enable `no_safe_boot`, `no_debugging_features`, `no_factory_reset`, `no_add_user`; test each (try safe boot, dev options, in-Settings reset) | Each takes effect. **Recovery-mode wipe still escapes — this is EXPECTED**, it documents the detect-not-prevent boundary (see concept §7), not a failure | ☐ PASS ☐ FAIL |
| **T9** | **Reboot persistence** — the Shizuku contrast | Reboot device; re-check DO status, restrictions, and that a foreground service / boot receiver can re-arm | DO + all restrictions survive reboot; nothing needs re-granting | ☐ PASS ☐ FAIL |
| **T10** | **Heartbeat feasibility** — the audit-gap source | Confirm the DPC can run a persistent foreground service that logs continuously across screen-off/idle | Continuous logging survives doze/idle — the signal whose *absence* is the parent-side tell | ☐ PASS ☐ FAIL |
| **T11** | *(optional)* **FCM / Play Services separability** | In kid profile, check whether sandboxed Play **Services** (not Store) is needed for a straggler's push, and that it's independently toggleable | Push works with it, app still works without it (no push). Informational only | ☐ PASS ☐ FAIL |

## Go / No-Go gate

**Hard gate — all must PASS or the architecture doesn't hold:**

- **T0** (provision), **T1** (install lockdown), **T2** (silent DO install),
  **T4 or T5** (a working app gate), **T6** (usage signal), **T9** (reboot persistence).

**Should pass — degrades the product if not, but not fatal:**

- **T7** (LockTask), **T8** (hardening), **T10** (heartbeat).

**Informational:** T3 detail, T11.

> Decision rule: **all hard-gate tests green → the [`concept.md`](concept.md) port is
> sound; feasibility is confirmed and a build can be scoped.** Any hard-gate red → that
> specific capability is the blocker to solve (or route around) *before* writing Charter
> code — not after.

## If something fails — first-look fallbacks

- **T1 partial** (one channel slips the restriction): note *which*; may need an additional
  restriction or `setApplicationHidden` on that store's package as a belt-and-suspenders.
- **T4/T5 unresponsive** (gate lags real use): the budget loop's reaction latency becomes a
  tuning problem, not a blocker — confirm *one* of the two is crisp enough.
- **T6 restricted** (usage stats unreadable): the budget has no input — this is a genuine
  blocker; investigate whether the DO can self-grant usage access on this build before
  proceeding.
- **T9 fails** (DO/restrictions don't survive reboot): that would echo the Shizuku failure
  mode and undermines the whole foundation — stop and resolve before anything else.
