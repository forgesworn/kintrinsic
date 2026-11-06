# Battery & responsiveness

Ward relay rounds open fresh connections; the guardian carrier holds WebSockets
open. Settings affect network work only. Enforcement remains at 2 seconds while
interactive and 60 seconds while dark; local-link monitoring remains at 3/30
seconds and liveness alarms at 60 seconds (subject to Android Doze limits),
except on Longer battery life with the screen off (see below).
Already received local changes still trigger enforcement promptly. Widgets
refresh while interactive and catch up on wake.

## Ward phone

In the guardian console, open the ward's Settings → Devices → the Android
phone → Battery & responsiveness. Advanced controls start collapsed.

| Preset | Screen off | Ordinary use | Request fast-check window | Opening boost |
|---|---:|---:|---:|---|
| Longer battery life | 600s | 120s | 60s | Off |
| Balanced (recommended) | 300s | 60s | 120s | On |
| Faster responses | 120s | 30s | 180s | On |

### What Longer battery life also does while the phone is asleep

The ward keeps a wake-up alarm that makes sure it is still running and has
checked its rules. It normally goes off every minute, day and night, and on a
ward's phone that is the most frequent thing waking it. On Longer battery
life, **while the screen is off**, the alarm goes off every 5 minutes instead.
Every other preset, and any time the screen is on, keeps the one-minute alarm.

In plain terms for a guardian:

- While the ward's phone is asleep, a change you send can take up to about 10
  minutes to arrive and take effect. The relay is only checked every 10 minutes
  with the screen off, and that check now waits for the next wake-up, which can
  be 5 minutes apart. On your home Wi-Fi (the local link) it is a few minutes.
- Times you have already set (bedtime, school hours, the end of extra time you
  granted, the end of music allowed to finish) still start within about a
  minute of when they should. The phone knows when the next one is due and sets
  its wake-up for that moment.
- When the screen comes on, or the phone is unlocked, it checks the rules
  straight away and goes back to the one-minute wake-up.
- Changing the setting takes effect at once; there is nothing to restart.

How it works: with the screen off, the next alarm is due at the earlier of 5
minutes and the next moment the rules change. The ward finds that moment by
asking its rule engine what it would enforce one, two, up to five minutes from
now (lock state, named time, app lists and per-app hours, buckets, tethering,
install window) and narrowing any difference down to about two seconds. It also
reads the end of an out-of-hours extension (not the screen-time budget, which
does not run down while the screen is off), of a debugging window and of a
listening grace directly. An alarm pulled forward to such a moment always runs
a check, even if one ran a few seconds earlier. If the rules cannot be read, or
a check fails, the alarm stays at one minute. This does not change
Doze allow-listing, the foreground service or relay pacing.

Two consequences to know about. The stuck-tick watchdog runs from the alarm, so a
wedged enforcement tick can take up to 5 minutes longer to be noticed. And the
core credits at most 300 seconds of elapsed time per tick, so a dark gap that
overshoots 5 minutes slightly under-counts an out-of-hours extension that is
burning wall-clock time, by seconds, in the ward's favour.

Fast checks wait 5 seconds **after a completed round**. Opening checks last
15 seconds, with a one-minute cooldown and a 30-second absence requirement
between activity sessions, avoiding renewal by overlapping activities or
configuration changes. Lock visibility alone does not maintain fast checks.
Opening or waking requests a prompt refresh, coalesced with running work or a round completed in the last five seconds.
Successful request submission starts an independent monotonic deadline for
that request, even if the ward leaves the GUI. Terminal answers remove only
the relevant request. Missing/stale pending records never create a boost.
Screen off always wins; an unexpired request window resumes on wake without
extending its original deadline. Process death discards temporary windows;
service recreation in the same process retains only original deadlines.
Disabling a boost makes it ineffective without changing its expiry.

Create the signed battery code, confirming the device's current relay list.
Apply it within ten minutes in the ward's About this device → Change relays /
battery. This reuses the existing offline guardian-authorised configuration
transfer, including during relay outages. It is not automatic relay delivery.
Signing is not evidence of receipt or application. The ward displays its
effective mode and request-window status, without editable controls. The
console displays proposed values, never an invented applied state.

The signed kind-31118 configuration is machine-bound, guardian-verified and
replay-protected. It persists beside that device's pairing. Relay-only edits
preserve its battery choice. Older pairings default to Balanced; malformed
local battery metadata falls back without damaging the pairing identity.
Invalid signed values are rejected before either settings or relays change.
Older ward APKs reject battery codes safely and need updating. Linux scheduling
is unchanged. No pairing-store synchronisation or consumer transport bypass
is introduced. See [the contract](../spec/contract.md).

## This guardian phone

Settings → Battery & responsiveness — this guardian phone changes the outage
retry ceiling: 600 seconds for Longer battery life, 300 for Balanced, 60 for
Faster responses. Retries start at one second and double; only a socket that
held for a minute resets the backoff. Existing queued retries retain their
deadline; subsequent failures use the selected ceiling. Repeated starts cannot
bypass a queued retry. The choice is validated and persisted in carrier-private
preferences, independently of every ward's configuration.

All modes retain background subscriptions, emergency notifications, local
delivery and four-minute WebSocket pings. Outage retries can delay requests
and emergency alerts. There is no delivery deadline. The native bridge is
feature-detected; older APKs and browsers do not show ineffective controls.
Restore recommended defaults resets the effective configuration.

## Verification and measurement

Run guardian console lint, typecheck, build and tests, both Android assemblies,
unit suites and lint. Build JNI with `android/scripts/build-jni.sh release`
after sourcing `~/Android/env.sh`; Gradle alone does not rebuild Rust. Host JNI
tests require `cargo test --lib --no-default-features --features
mock,charter-sys/real-os`. The real-os feature supplies persistence even for
injected-relay tests; examples require live relay features. Run the separate
core and Linux workspace checks in AGENTS.md. Regenerate both vector scripts;
only the new battery fixture should differ. Queue integration tests exercise
the production loop with an injected serial clock/queue, overlapping triggers,
dark priority, settings, stopping and watchdog re-arming.

Use a managed emulator for enforcement. Never enrol or restrict an ordinary
test phone. Install rebuilt APKs, capture `adb logcat -s CharterService
CarrierService`, and exercise screen off/on, opening/leaving the GUI,
unanswered requests, two requests answered separately, relay outage/recovery
and process/service recreation. Apply a signed code bound to that emulator,
then verify the effective mode after recreation. Local answers should still
apply while relay fallback is slow. Check carrier notifications, including
emergency audits, with the GUI closed.

Under identical relay/network conditions, compare poll-completion counts,
connections, reconnect attempts, wake/CPU activity (`dumpsys batterystats`)
and approval-to-application latency against a clean baseline. Separate awake,
dark, outage and active-wait periods. Balanced reduces nominal ordinary
fallback frequency from the original 15 seconds to 60, and dark frequency from
120 to 300 seconds. These are scheduling expectations, not measured battery
savings. Representative physical battery/radio measurements remain necessary;
host tests and emulator timing cannot establish them.

## Verification record (3 November 2026)

- Both debug APKs assembled, including rebuilt ward JNI for arm64-v8a and
  x86_64. The release native build passed its no-mock and 16 KB alignment gates.
- Android host suites: 332 ward and 66 carrier tests passed. JNI: 181 tests
  passed, including signed battery application and restoration after restart.
- Guardian console: lint (42 existing warnings, no errors), typecheck, build
  and 2,041 tests passed. After the final visual spacing changes, the 61 tests
  covering settings, signer and device configuration passed again. Root SDK
  lint/typecheck/build and 66 tests passed.
- Core and Linux workspace format, clippy and default tests passed. The
  charter-sys real-relay and charterd real-feature tests passed (three existing
  Linux tests ignored). Both vector generators ran; existing vectors did not
  drift. A new shared battery configuration vector was added.
- Chrome at 412px width rendered both settings surfaces. Guardian bridge
  saving and ward preset/custom/default transitions were exercised.
- Android carrier lint: no errors, 18 warnings. Ward lint remains blocked by
  10 existing errors (MissingPermission, ProtectedPermissions,
  PermissionImpliesUnsupportedChromeOsHardware and QueryAllPackagesPermission)
  on unchanged permission calls/manifest declarations; 124 warnings. These
  findings were compared with the pre-existing source, not suppressed.
- A managed charter-ci emulator passed the ABI and signed-clause suspension /
  install-lockdown tests. ADB became unavailable during the third existing
  install-lock test, so the full instrumentation suite and further device
  lifecycle scenarios are unverified. No physical phone was modified. Initial
  emulator boot and native builds also encountered host resource pressure;
  generated Cargo outputs were cleaned and native builds successfully retried.

There is no measured battery saving, representative radio/wake comparison,
background notification hardware check or approval-latency result from this
session. Those remain the physical acceptance measurements described above.
No release was published and no family device received a deployment.

## Published builds

Ward 0.7.2 (version code 51) and guardian 0.2.2 (version code 25) were
subsequently released from merged commit `a0242e66c6afe4e90dd0daccb851158d8d7daa62`.
GitHub-hosted builds passed provenance, SHA-256, signing-certificate and rotation
lineage verification before publication. Both artifacts have a verified direct
Blossom mirror, and all three update relays accepted their signed announcements.
The guardian web deployment and merged-commit Rust CI passed. The physical
measurement and instrumentation limitations above still apply. Publication did
not install either APK on family devices.
