# Carrier APK — Emulator End-to-End Round (Evidence Log)

**Date:** 2026-07-22
**Rig:** `charter-ci` AVD (Android 16 / API 36, x86_64, headless, `-gpu off -memory 2048 -cores 2`), live relay `wss://relay.trotters.cc`, laptop-built `carrier-debug.apk`.

## What was proven

1. **Install + provision.** The AVD arrived with the ward app as Device Owner and
   `DISALLOW_INSTALL_APPS` active — which *blocked the carrier install* (an
   accidental live proof of #44's enforcement). Removed the test DO
   (`dpm remove-active-admin`, dev/testOnly build), installed clean. Provisioned
   the carrier store directly via `run-as` with a throwaway guardian keypair
   (`336936c4…`; the deployed console predates the Task-5 bridge, so the
   in-WebView hand-off is exercised by its 3 vitest tests, not this round).

2. **The core loop — publish → wake, app backgrounded.**
   - `fire_ask` published a real kind-31111 REQUEST wrap:
     `reqId=13e48930… (10 min) … [("wss://relay.trotters.cc", Ok)]`
   - ~1 s later the backgrounded service classified it:
     `18:48:26.363 I CarrierService: ward request 13e48930… (time.extend)`
   - Notification posted: channel `carrier.approvals`, `importance=4` (HIGH),
     `mIsInterruptive=true`, default notification sound, category `msg`,
     title "Charter request", text **"Your ward asks for 10 more minutes"**.

3. **Dedupe across restart (relay replay).** `force-stop` → relaunch →
   socket reopened (`18:48:50 open wss://relay.trotters.cc`), the 48 h-window
   resubscribe replayed the stored wrap, and the persisted seen-reqId LRU kept
   it silent: **0** approvals notifications after restart.

4. **Delivery still live post-restart (control).** A second, fresh ask
   (`reqId=2ee2b8b8…`, 25 min) alerted immediately —
   "Your ward asks for 25 more minutes" — final approvals count exactly **1**
   (the new ask; the replayed one stayed suppressed). Dedupe proven, not a
   delivery failure.

5. **Tap route.** `am start … -e route approvals` delivered to the running
   singleTask instance (`onNewIntent` → hash navigation), activity resumed,
   crash buffer empty.

6. **Release artifact.** `carrier-release.apk` (9,810,441 bytes,
   debug-key-signed alpha per the ward-app convention):
   `sha256 476e4cf9f8df06d815ba9379705997ec2cd950c4d7c5a7f2fd6267a1a3e45146`
   Guardian `.so` release-built for arm64-v8a + x86_64, 16 KB-aligned.

## Environment note

The headless emulator was OOM-killed by the host (16 GB laptop, exit 137)
*after* all evidence above was captured, while the release build ran. Same
thing killed the first boot attempt mid-setup. Not a product issue; budget
RAM for future emulator rounds (stop Gradle daemons first).

## NOT yet proven (hardware-round items — need a real phone)

- Doze / deep-idle survival and reconnect latency on real silicon
- Swipe-away-from-recents → START_STICKY restart behaviour
- Boot-completed re-arm (BootReceiver) on a real boot
- Lockscreen presentation: full-screen intent vs heads-up on Android 14+
  (and whether the dedicated keyguard-safe alert activity is needed)
- The real in-WebView provision hand-off against the deployed console
  (needs `apps/charter-app` deployed from main first)
- Battery cost over a multi-day soak
- POST_NOTIFICATIONS / battery-exemption ask UX (granted via adb here)
