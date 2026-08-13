# Changelog

## 0.84.0 (2026-08-12)

### Features

- stage hash-verified deb updates from signed relay events (charterd)
- in-app self-update — Blossom download, sha256 pin, PackageInstaller confirm (carrier)
- publish signed kind-30063 release events + Blossom mirrors from the publish scripts (release)
- update manifests come from signed relay events, origin JSON demoted to fallback (app)
- verify + adapt kind-30063 release events into UpdateManifest shape (app)
- kind-30063 software release events verified against a pinned release key (verify)
- nostr release-key ceremony script + custody notes (release)

### Bug Fixes

- verify mirrors with a redirect-refusing full-body GET, resolving one CDN hop (release)



## 0.83.0 (2026-08-10)

### Features

- bundle the guardian console into the APK (D1, decentralized stack) (carrier)

### Bug Fixes

- alert once per emergency unlock, not once per redelivery (mycharter)



## 0.82.0 (2026-08-07)

### Features

- show whose key this phone is about to pin, before it pins it (ward)



## 0.81.1 (2026-08-07)

### Bug Fixes

- the `real`-gated test fixture missed the new caller_uid field (charterd)



## 0.81.0 (2026-08-07)

### Features

- keep the guardian key sealed, and say what you're signing (mycharter)
- close the safe-mode holiday, and count the boots we slept through (ward)
- choose the time model, and let a website carry a policy (mycharter)
- charge by the open-window set, not the focused window (charterd)
- the named-costs time model — no baseline, no waiver, count once (core)
- a website can carry a policy — the `site:` identity and a free flag (core)

### Bug Fixes

- a half-written vault must leave no trace (mycharter)
- remote unpair works on Linux, and a sibling can't read your asks (warden)
- a release build no longer signs itself with the debug key (android)
- a phone must PROVE it was paired before it can be claimed (mycharter)



## 0.80.2 (2026-08-06)

### Bug Fixes

- check for a newer page when the app returns to the foreground (mycharter)



## 0.80.1 (2026-08-06)

### Bug Fixes

- 0.1.6 — a freshly-installed app no longer calls itself out of date (mycharter)



## 0.80.0 (2026-08-04)

### Features

- dismiss a repeat ask without answering it (mycharter)
- a denial never renders "Approve"; locality from signer kind (mycharter)
- confirm gate knows which decision it is, skips for local (mycharter)
- push the child/device roster to the carrier bridge (mycharter)
- resolve a device pubkey to child+device name in notifications (carrier)

### Bug Fixes

- a ward cannot turn a dormant device on herself (mycharter)
- gender-neutral copy in Approvals dismiss + Always-available help (mycharter)



## 0.79.0 (2026-08-04)

### Features

- surface out-of-hours use to guardian and ward alike
- Always available section — name the apps open at any hour (mycharter)
- GrantAlwaysAvailable — apps open at any hour, standing or expiring (wire)
- out-of-hours counter — a fact about the day, not a pool of time (usage)
- an Open row for the apps open at any hour (shade)
- resolve alwaysavailable each tick; level-trigger the LockTask allowlist (warden)
- always_available_view — which apps may be opened through a lock (jni)
- alwaysavailable clause — apps open at any hour, by name (proto)

### Bug Fixes

- shade's Open row now consults the app gate before offering a button (I3) (mycharter)
- give the out-of-hours line its own scope word (I2) (mycharter)
- guard the guardian's out-of-hours week line against stale devices (C1) (mycharter)
- make the out-of-hours vector reader's guard check completeness (mycharter)
- out-of-hours line can no longer contradict the week card (mycharter)
- surface always-available's too-old note, document the clause in contract.md (mycharter)
- always-available parity gate, drop lint suppressions, versionCode 39 (mycharter)
- out-of-hours must not credit the unlocked→locked transition tick (usage)
- only cache the LockTask allowlist on a confirmed DPM success (warden)
- an exemption may not beat a standing block; add alwaysAvailable seam (enforce)



## 0.78.0 (2026-08-03)

### Features

- honest attribution — name the game, and say what we can't see
- unrecognised time, user-installed marks, Minecraft launch signature (mycharter)
- count the time Charter can't identify, and show what the ward installed (charterd)
- attribute and stop a game by its command line (charterd)
- cmdline: identity form — name the game, not the launcher (core)

### Bug Fixes

- match charterd's day-boundary tz, defence-in-depth strip, re-attach on re-entering Counted (mycharter)
- restrict Minecraft signature to Counted, fix parity/userInstalled copy, day-guard unrecognised time (mycharter)
- a gifted minute must move the child's clock; tolerate cmdline identities on Android (ward)
- stat the kernel's exe link for ownership, and drop the limb that kept accusing children (charterd)
- suppress only on what the kernel stat'd, and fail learning closed on an unknown inventory (charterd)
- trust only what the kernel resolves, and let an editor open a child's homework (charterd)
- let flatpaks back into the inventory, and stop the unrecognised counter accusing ordinary use (charterd)
- close the FIFO/symlink DoS in app inventory scanning, redefine unrecognised time (charterd)
- gate site-app sanction on runtime ownership (C2), share exe-link reads (C3) (charterd)
- close the ungated flatpak/bare-name learning bypass (C1) (charterd)



## 0.77.0 (2026-08-03)

### Features

- segment the listening picker by device too (mycharter)
- segment app pickers by device (mycharter)



## 0.76.2 (2026-08-03)

### Bug Fixes

- a refresh must not steal the reader's scroll place; release 0.7.4 (console)



## 0.76.1 (2026-08-03)

### Bug Fixes

- clamp time.extend exp to the device's real validity cap (mycharter)
- close three guardian-side bugs from named-times hardware round (mycharter)



## 0.76.0 (2026-08-03)

### Features

- named times — one family time model (free/counted/on-request groups)
- group-aware asks, holds-from-asks, give-to-group, group progress (mycharter)
- Named times — one editor for free/counted/on-request groups (mycharter)
- weekly buckets v-rule, group gifts/asks, app.open, askFirst, group progress (mycharter-wire)
- ward mirror group rows + group/on-request asks (android)
- buckets enforced — meter, weekly walls, suspend-on-spend, group gifts (android)
- group rows, group asks, ask-to-open (linux-ward)
- weekly group walls, group gifts/extends, STATUS group progress (charterd)
- route per-group time.extend grants through the enactor (spine)
- bucket asks, app.open op, askFirst hint, STATUS group progress (proto)
- per-group additive pools; gift learns groupId (extension)
- week-keyed bucket meters beside the day meters (usage)
- weekly allowances — daily and/or weekly caps per group (buckets)

### Bug Fixes

- collapsed Apps row must count what the wire actually signs (named-times)
- move F1 allowlist strip from applyAppsFragment to appsToGrant (N1 regression) (named-times)
- close allowlist askFirst fail-open, paused progress wall, weekly bucket bar (final review F1-F3) (named-times)
- the "you can open it" copy must not outlive the hold (minor a) (android)
- app.open minutesGranted + clause-before-grant ordering (N1, N2) (mycharter)
- app.open GRANT shape, device-override composition (C1d, C2) (mycharter)
- a grant is not forever — re-offer "ask again" after Enacted (android)
- register AppOpenEnactor; fix Android's dead submit path (linux,android)
- app.open gets a real GRANT — {pkg, minutesGranted} (C1) (core)
- Named times review round 2 — 2 critical fixes + 1 identity fix (mycharter)
- Named times review round 1 — 2 critical + 4 important fixes (mycharter)
- a denied group/app-open ask is re-askable, not permanent (android)
- buckets-only wards meter/enforce; bucket week rolls on its own tz (android)
- binding bucket remainder, validated gift group routing (charterd)
- honour the buckets clause's own week-roll policy; fail-soft dead branch (spine)



## 0.75.0 (2026-08-02)

### Features

- say so when a control does nothing on one of the ward's devices (parity)

### Bug Fixes

- close five silent-failure bugs found in the full-codebase audit (audit)



## 0.74.0 (2026-08-02)

### Features

- enforce the apps clause — standing lists AND holds bind a laptop (linux)



## 0.73.0 (2026-08-02)

### Features

- hold an app open or shut for a while, and let it end itself (apps)



## 0.72.0 (2026-08-02)

### Features

- say when a device has no browser to open its learning sites in
- three measured sites, and a way to add your own (mycharter)
- Chromium becomes a site-app runtime, not a browser (charterd)
- site-app launch identity, rendered and checked in one place (charterd)

### Bug Fixes

- blocking Google Chrome was silently inert (charterd)



## 0.71.0 (2026-08-02)

### Features

- a tray menu Mint draws itself (linux)



## 0.70.3 (2026-08-01)

### Bug Fixes

- stop running a second relay client in the background (mycharter)
- the phone rests when the screen is off (ward)



## 0.70.2 (2026-08-01)

### Bug Fixes

- a split rule now offers each device only its own apps (mycharter)



## 0.70.1 (2026-08-01)

### Bug Fixes

- the per-device switch said the opposite of what was true (mycharter)



## 0.70.0 (2026-08-01)

### Features

- each section is now somewhere you can tell you are (mycharter)



## 0.69.1 (2026-08-01)

### Bug Fixes

- the tray no longer dies when the desktop isn't ready yet (linux)



## 0.69.0 (2026-08-01)

### Features

- a tray clock that says how much is left, and opens the whole picture



## 0.68.0 (2026-07-31)

### Features

- the ward's computer says what it's doing, and a parent can change it there



## 0.67.1 (2026-07-31)

### Bug Fixes

- a grant to an overdrawn ward buys the minutes it says (enforcer)



## 0.67.0 (2026-07-31)

### Features

- a device says what limits it is enforcing, so the phone stops lying by omission



## 0.66.0 (2026-07-31)

### Features

- say what's true about a ward before giving them time (mycharter)

### Bug Fixes

- granted time is spent, so the lock actually comes back (enforcer)



## 0.65.0 (2026-07-31)

### Features

- the pairing code counts down and vanishes when it expires (console)

### Bug Fixes

- let the daemon write /etc/charter so a scanned pairing can land (charterd)



## 0.64.3 (2026-07-31)

### Bug Fixes

- send the pair offer on a deliberate press, and never hide a refused approval (mycharter)



## 0.64.2 (2026-07-31)

### Bug Fixes

- never show a pairing QR that has no token in it (console)



## 0.64.1 (2026-07-31)

### Bug Fixes

- drop the unused url parameter from the mock signer (lint)



## 0.64.0 (2026-07-31)

### Features

- send a pair offer after scanning a computer's QR (mycharter)
- scan-to-pair QR, wait-for-phone Connect page, optional child ID (console)
- unpaired wards listen for a pair offer and pin it (charterd)
- one-time pairing token + pair-offer acceptance rules (charterd)
- publish + poll PAIR_OFFER, surfacing the authenticated seal author (wire)
- PAIR_OFFER kind 31117 + payload with constant-time token compare (wire)



## 0.63.1 (2026-07-31)

### Bug Fixes

- tell the ward how long they actually have



## 0.63.0 (2026-07-30)

### Features

- a window a ward can see, and an account of what came through it



## 0.62.1 (2026-07-30)

### Bug Fixes

- a reload no longer forgets that installs are open



## 0.62.0 (2026-07-30)

### Features

- a guardian can close an install window early



## 0.61.0 (2026-07-30)

### Features

- a ward can update their apps, when their guardian says so



## 0.60.0 (2026-07-29)

### Features

- a story may finish — listening through the lock



## 0.59.0 (2026-07-29)

### Features

- a ward's colour is editable, not a one-time choice (mycharter)



## 0.58.1 (2026-07-29)

### Bug Fixes

- say which ward, and mean it when you say "Connected" (mycharter)



## 0.58.0 (2026-07-29)

### Features

- every device keeps a way out, unless a guardian removes it



## 0.57.2 (2026-07-29)

### Bug Fixes

- the shape claim was really a timezone claim (test)



## 0.57.1 (2026-07-29)

### Bug Fixes

- three ways the dormant posture lied about itself



## 0.57.0 (2026-07-28)

### Features

- a device that is off until its guardian opens it



## 0.56.3 (2026-07-28)

### Bug Fixes

- never offer a call a device cannot place (android)



## 0.56.2 (2026-07-28)

### Bug Fixes

- don't send a parent to the phone they're holding (charter-app)



## 0.56.1 (2026-07-28)

### Bug Fixes

- never ship a library the release build did not produce (build)



## 0.56.0 (2026-07-28)

### Features

- one visual language for the family's screens (android)



## 0.55.2 (2026-07-28)

### Bug Fixes

- a switched-off device must not veto the guardian (charter-app)



## 0.55.1 (2026-07-28)

### Bug Fixes

- say the time left the way the ward's own app says it (charter-app)



## 0.55.0 (2026-07-28)

### Features

- a claimed phone gets the standing charter by itself (charter-app)



## 0.54.1 (2026-07-28)

### Bug Fixes

- hold the D-Bus connection open — device-only boxes served for one line (linux)



## 0.54.0 (2026-07-28)

### Features

- a tray for the ward — time left at a glance, and a way to ask (linux)



## 0.53.2 (2026-07-28)

### Bug Fixes

- the parent-facing cable setup switches on the time meter too (provision)



## 0.53.1 (2026-07-28)

### Bug Fixes

- the stand-down's lever must reach every ward, honestly (charter-app)
- a failed display-manager restart must not go quiet forever (linux)
- a ward with no rules is still stoppable, and the shade says why (android)
- a lift must free the ward, even on a slow clock (core)



## 0.53.0 (2026-07-27)

### Features

- set allowed hours for the whole week at once (charter-app)



## 0.52.1 (2026-07-27)

### Bug Fixes

- a lock that ends its own session must not take the display (linux)



## 0.52.0 (2026-07-27)

### Features

- charterd enforces the stand-down too (linux)



## 0.51.1 (2026-07-27)

### Bug Fixes

- a locked device speaks for the ward, not the chattiest one (charter-app)



## 0.51.0 (2026-07-27)

### Features

- Finish now — the inverse of Give time (charter-app)



## 0.50.0 (2026-07-27)

### Features

- enforce the stand-down, and make the ward's minute real (android)



## 0.49.0 (2026-07-27)

### Features

- guardian stand-down — "finish up now", then done for today (core)



## 0.48.0 (2026-07-27)

### Features

- keep the ward switcher on screen while setting limits (charter-app)



## 0.47.2 (2026-07-27)

### Bug Fixes

- don't lose a ward's ask because her phone isn't set up yet (charter-app)



## 0.47.1 (2026-07-27)

### Bug Fixes

- offer an unclaimed phone once, not under every ward (charter-app)



## 0.47.0 (2026-07-27)

### Features

- offer a phone that paired with you but isn't set up here (charter-app)



## 0.46.4 (2026-07-27)

### Bug Fixes

- a rule that never travelled must not read as done (charter-app)



## 0.46.3 (2026-07-27)

### Bug Fixes

- don't say the minutes were sent when nothing was sent (charter-app)



## 0.46.2 (2026-07-27)

### Bug Fixes

- grant usage access at the cable, and verify it took (provision)



## 0.46.1 (2026-07-26)

### Bug Fixes

- don't offer Give time to a phone that can't accept it (charter-app)



## 0.46.0 (2026-07-26)

### Features

- tell the ward the answer, wherever they are (android)



## 0.45.0 (2026-07-26)

### Features

- give a child time without them having to ask



## 0.44.2 (2026-07-26)

### Bug Fixes

- a denied ask still read "Asked! Your guardian will see it shortly" (android)



## 0.44.1 (2026-07-26)

### Bug Fixes

- a release published while the app was open was invisible (charter-app)



## 0.44.0 (2026-07-26)

### Features

- the ward holds the switch for their own guest hotspot (android)



## 0.43.1 (2026-07-26)

### Bug Fixes

- the filtered guest hotspot could never start on a real phone (android)



## 0.43.0 (2026-07-26)

### Features

- a stuck update is visible, and a stuck phone is rescuable



## 0.42.1 (2026-07-26)

### Bug Fixes

- self-update died at boot — staging path captured before unlock (android)



## 0.42.0 (2026-07-26)

### Features

- the guardian can see what a paired laptop is running



## 0.41.1 (2026-07-26)

### Bug Fixes

- stop deleting APKs a phone may still be fetching (release)



## 0.41.0 (2026-07-25)

### Features

- a torch on the shade, switchable by the guardian (android)



## 0.40.0 (2026-07-25)

### Features

- time buckets — "Play is an hour a day", end to end

### Bug Fixes

- a bucket follows the game a launcher started (linux)



## 0.39.0 (2026-07-25)

### Features

- the per-device rules UI — split any control, per device (app)
- per-device rules engine + say what a child's devices actually are (app)



## 0.38.0 (2026-07-25)

### Features

- the shade carries the phone's vital signs — clock, charge, reach (android)



## 0.37.0 (2026-07-24)

### Features

- MyCharter lifeline v2 knobs behind a self-lifting ship-order guard (app)
- alert the guardian the moment a break-glass override lands (carrier)
- break-glass override — instant, offline-capable, loudly transparent (android)
- lifeline v2 shade — 5 numbers, regional emergency entry, hold-to-call friction (android)
- JNI surfaces lifeline v2 (numbers + emergencyServices + breakGlass) (android)
- lifeline v2 wire — 5 numbers, emergencyServices flag, breakGlass config (acceptance first) (core)



## 0.36.2 (2026-07-24)

### Bug Fixes

- in-call passthrough + End-call bar — the trapped-call P0 (0.3.1) (android)



## 0.36.1 (2026-07-24)

### Bug Fixes

- out-of-window schedule extensions burn wall-clock and re-lock (on-device fail-open) (core)



## 0.36.0 (2026-07-24)

### Features

- every notification taps through to its app (decented's request) (android)



## 0.35.0 (2026-07-24)

### Features

- union weekly picture — 'both at once' segment + softened overdraft wash; B3b docs (app)
- USAGE_SYNC 31115 publisher — per-device elsewhere views, change-gated (app)
- STATUS activeMinutesToday intake + minute-journal history + union weekly totals (app)
- TS MinuteSet — byte-identical to Rust (frozen vectors) (app)



## 0.34.0 (2026-07-24)

### Features

- ingest USAGE_SYNC → pooled budget enforcement + STATUS activeMinutesToday (warden)
- pooled quota — cap − |own ∪ elsewhere|, scalar fallback, period-roll safe (core)
- USAGE_SYNC transport poll + broker ingest via reserved store key (core)
- verify_usage_sync — guardian-pinned, ts-monotonic, fail-closed bitmap (core)
- USAGE_SYNC 31115 wire payload + STATUS activeMinutesToday (frozen vectors) (core)
- UsageLedger journals per-minute activity (survives snapshot, clears on day roll) (core)
- MinuteSet 1440-bit day bitmap with frozen b64url vectors (core)



## 0.33.1 (2026-07-24)

### Bug Fixes

- SHA-pin Swatinem/rust-cache — unblocks the release strict-action-pins gate (ci)



## 0.33.0 (2026-07-24)

### Features

- the weekly picture (B1) — a child's screen-time week on Activity (app)



## 0.32.0 (2026-07-24)

### Features

- single canonical download place — in-app card points to charter.signet.you/download; add front-door sync script



## 0.31.2 (2026-07-24)

### Bug Fixes

- in-app camera works — CAMERA perm + WebChromeClient grants the WebView's getUserMedia (0.1.1) (carrier)



## 0.31.1 (2026-07-24)

### Bug Fixes

- status push now reaches the UI — accept the host object, not just a JSON string (0.3.3) (console)



## 0.31.0 (2026-07-23)

### Features

- CHARTER_CONSOLE_DEBUG status line — limits-dir readability + child count for field diagnosis (console)



## 0.30.5 (2026-07-23)

### Bug Fixes

- service worker no longer hijacks artifact downloads as SPA navigations (charter-app)



## 0.30.4 (2026-07-23)

### Bug Fixes

- the app finally PAINTS — attach the webview via build_gtk, not the raw X11 handle (0.3.2) (console)



## 0.30.3 (2026-07-23)

### Bug Fixes

- auto-reload once when a new version takes control — kills the close-reopen-twice update dance (charter-app)



## 0.30.2 (2026-07-23)

### Bug Fixes

- device row wraps on phone width; lifeline editor uses the app Button (charter-app)



## 0.30.1 (2026-07-23)

### Bug Fixes

- restored household no longer clobbered by the persist-on-change effect (charter-app)



## 0.30.0 (2026-07-23)

### Features

- v2 household backup — one blob restores key + children + devices + rules (charter-app)



## 0.29.0 (2026-07-23)

### Features

- ward 0.2.9 — tapping the time-left widget opens the Charter app (android)



## 0.28.0 (2026-07-23)

### Features

- ward 0.2.8 — hotspot credentials visible (notification + mirror), h/m/s time-left format (android)



## 0.27.1 (2026-07-23)

### Bug Fixes

- 'Rest of today' hotspot window is now selectable (charter-app)



## 0.27.0 (2026-07-23)

### Features

- 'Get the MyCharter app' download card on Home — manifest-driven, hidden inside the carrier (charter-app)



## 0.26.0 (2026-07-23)

### Features

- publish MyCharter carrier APK 0.1.0 — the guardian-side app goes live (release)



## 0.25.1 (2026-07-23)

### Bug Fixes

- tint the ward main window fully dark; document the deliberate non-directBoot carrier decision (D2 custody trade) (android)



## 0.25.0 (2026-07-22)

### Features

- live_guardian send-lifeline + fix JSON-null 'detail' rendering; Pixel 8 on-metal evidence (android)



## 0.24.0 (2026-07-22)

### Features

- ward mirror (D8) + communication lifeline (D9)
- lifeline authoring — guardian numbers clause (D9), Limits editor + wire (charter-app)
- lock-screen 'Call your guardian' (D9) + ward mirror screen + time-left widget (D8) (android)
- schedule_view (D8 mirror) via the shared spine composer + JNI (android)
- warden lifeline read-surface + charterLifeline JNI (fail-closed) (android)
- lifeline clause kind (store_key 9) + fail-closed LifelineBody (D9) (proto)



## 0.23.0 (2026-07-22)

### Features

- MyCharter carrier APK — always-on guardian notifications, no FCM (D5) (android)
- fire_ask example — one-shot test REQUEST for carrier e2e rounds (carrier)
- always-on foreground listener — websocket sub, classify, urgent ward-request notifications (carrier)
- NIP-01 relay framing with 48h jitter-safe since window (carrier)
- hand the guardian key to the MyCharter carrier shell when present (charter-app)
- provision store + CharterCarrier JS bridge (strict payload parse) (carrier)
- :carrier module — MyCharter WebView shell with origin-locked navigation (carrier)
- guardian JNI surface + .so build script (16 KB gate) (carrier)
- charter-guardian-jni crate — pure gift-wrap classify core (carrier)



## 0.22.4 (2026-07-22)

### Bug Fixes

- ship the self-update artifact as a non-testOnly release build (#44) (android)



## 0.22.3 (2026-07-22)

### Bug Fixes

- a stale RELEASE from before a re-pair no longer re-unpairs the device (#49 sibling) (android)
- DISALLOW_INSTALL_APPS blocks the Device Owner's own installer too (#44) (android)



## 0.22.2 (2026-07-22)

### Bug Fixes

- refuse an unpinnable Charter self-approval instead of silently skipping (#44) (mycharter)



## 0.22.1 (2026-07-22)

### Bug Fixes

- pin versionCode when approving Charter's own install ask (#44) (mycharter)



## 0.22.0 (2026-07-22)

### Features

- catalog Charter itself — the self-update bootstrap approval (#44) (mycharter)



## 0.21.1 (2026-07-22)

### Bug Fixes

- versioned .apk.txt artifact path — vhost + cache-skew workaround (#44, #46) (release)



## 0.21.0 (2026-07-22)

### Features

- publish-apk script — the self-update artifact pipeline (#44) (release)
- see device versions + one-tap Update Charter (#44) (mycharter)
- url-sourced installs — download, pin sha256, hand to the DO installer (#44) (android)
- update clause parks a url self-install when the device is behind (#44) (jni)
- devices report their app versionCode (#44) (status)
- update clause — guardian-directed self-update wire (#44) (proto)



## 0.20.2 (2026-07-21)

### Bug Fixes

- omit sub-minute usage from the ward lock screen (android)
- verify the usage-access self-grant instead of trusting it (android)



## 0.20.1 (2026-07-21)

### Bug Fixes

- harden the Charter Hotspot — egress guard + lifecycle races (tethering)



## 0.20.0 (2026-07-21)

### Features

- guardian UI + wire contract + cross-stack vectors (tethering)
- CharterHotspotService + controller wiring (tethering)
- default-off clause + Charter Hotspot filtering doorman (tethering)

### Bug Fixes

- close trailing-dot filter bypass + proxy socket-set leak (tethering)
- close review findings — IP-literal fail-open + robustness (tethering)
- tear down filtered hotspot on un-charter; avoid VPN tun iface (tethering)



## 0.19.0 (2026-07-20)

### Features

- Linux terminates blocked/out-of-window apps (per-app enforcement) (per-app)
- Android suspends blocked/out-of-window apps (per-app enforcement) (per-app)
- MyCharter emits the appRules clause + real allowed-hours editor (per-app)
- contract foundation for per-app control (block + allowed-hours) (per-app)



## 0.18.1 (2026-07-20)

### Bug Fixes

- make exec.allow actually bind the source path (was dead on real hw) (charterd)



## 0.18.0 (2026-07-20)

### Features

- offline guardian unlock — challenge/response, no PIN, no network (unlock)



## 0.17.1 (2026-07-20)

### Bug Fixes

- harden auth, pairing, lock, and CI across the stack (review)



## 0.17.0 (2026-07-19)

### Features

- device-only learning — the no-phone path (0.3.1) (linux/console)
- device-only learning fallback in child config + resolution (spine)
- Learning section — time-free apps with verified Khan pin (app)
- learning bucket in the loop + status + app inventory (charterd)
- pinned learning-app launchers (learning_apps enactor) (charterd)
- foreground bucket attribution (focus.rs) (charterd)
- bucketed tick + learning cap fallback (spine)
- learning clause v1 + golden vectors (proto)
- learning bucket in the usage ledger (schedule)
- one professional app — charter-console replaces five dialogs (linux)
- real download button in the setup guide (charter-app)
- normie-ready computer onboarding (app,linux)
- host the Linux installer for download (charter-app)
- wire release signing to pipeline secrets, not a local key (android)
- charter-provision.sh scripted fallback (DO + cable pair) (android/provision)
- "set up with a cable" UI — WebUSB provisioning end-to-end (pwa/provision)
- same-origin release APK fetch + sha256 for transparency (pwa/provision)
- preflight -> install -> set-device-owner -> cable-pair sequence (pwa/provision)
- typed WebUSB adb session wrapper (connect/shell/push) (pwa/provision)
- pin + apply the DNS filter from WardenController (fail-closed) (android/web)
- CharterVpnService — split-tunnel DNS filter (block/rewrite/relay) (android/web)
- DnsResolver — plan -> block/rewrite/passthrough (suffix-aware) (android/web)
- minimal DNS wire codec (parse question, NXDOMAIN, A/AAAA answer) (android/web)
- CharterCore.dnsPlan() — typed DNS plan across the JNI boundary (android/web)
- charterDnsPlan() export — surface the DNS plan to Kotlin (jni/web)
- Warden::web_dns_plan() — evaluate content clause -> DnsFilterPlan (jni/web)
- phone-reported app inventory — pick apps by name (D3 UX) (apps)
- author standing per-app policy (D3 v1) (mycharter/apps)
- standing per-app control (D3 v1) — wire + phone enforcement (android/apps)
- parent-authoritative web-content authoring (v1) (mycharter/web)
- ward-facing "Ask for an app" screen (android/ui)
- a "Not now" on an install ask reaches the phone (mycharter/install)
- publish real install.apk grants — catalog-pinned cert (mycharter/install)
- guardian-approved install.apk — DO silent install, cert-pinned (android/install)
- parent-gated unpair/re-pair via a guardian-signed device RELEASE
- encrypted guardian-key backup + restore — kill the key-loss cliff (charter-app)
- grant-arrival notification; fix sign-crash on non-hex device keys (app+android)
- the offline outbound spool — asks made offline reach the guardian (port-spec §2.2) (android)
- install.apk — the additive op for guardian-approved Android installs (port-spec §5.4) (contract)
- time warnings, ask outcomes, live child status — the humane layer (android+app)
- the ask-for-more-time loop — locked phones are no longer dead ends (android)
- one-scan QR onboarding — no typing, no camera permission, either direction (pairing)

### Bug Fixes

- pause the screen-time meter during suspend/hibernate (linux/time)
- let a ward shut down, sleep and hibernate (linux/polkit)
- a scannable laptop QR + a real camera scanner in MyCharter (pairing)
- serve the installer under its real name (charter-app)
- serve the installer under a whitelisted extension (charter-app)
- always-on VPN without lockdown (fail-soft) — lockdown breaks all internet (android/web)
- gate the DNS-filter pin on a paired ward (I17 inert-until-charter) (android/web)
- address final-review findings (stderr, signer gate, one-tap copy) (provision)
- fail closed on a content-clause envelope read error (jni/web)
- empty the app catalog — drop a fabricated entry (mycharter/install)



## charter (Linux warden) 0.3.1 (2026-07-18)

### Features

- **device-only learning** — the no-phone path: "Learning time" section in the
  on-computer Charter app (Khan Academy toggle, verified pin) via new
  `charter-settings --set-learning/--clear-learning` (GrantLearning JSON on
  stdin, fail-closed); ChildConfig carries a fail-soft learning field
- precedence per whole-child doctrine: a guardian learning clause always wins;
  a time-governing guardian suppresses device-only learning; an un-governed
  child uses the device list
- setup guide + setup.txt now explain Learning time


## charter (Linux warden) 0.3.0 (2026-07-17)

### Features

- **learning buckets** — guardian-designated learning apps are time-free:
  focused time credits a separate learning meter instead of draining the
  screen budget (learning clause v1, per-child; optional daily cap — past it,
  learning charges screen, never locks)
- site learning apps materialise as resolver-pinned Chromium app windows
  (Khan Academy seed with a MEASURED domain closure; the embed's
  "Watch on YouTube" walk-out dies at the resolver)
- native learning apps by root-owned exec identity or flatpak id; guardian-
  vouched own-project paths (advisory, labelled)
- foreground attribution via xprop → /proc identity (fail-closed to screen);
  STATUS + TimeLeft + `charter status` + MyCharter child card show
  learning-today; root-owned installed-app inventory published on STATUS
- MyCharter: Learning section in the child's rules (catalogue, program
  picker, own-project vouch, cap)

### Notes

- charter-console (device-only) learning editor is a fast-follow; the
  guardian-phone path is complete end-to-end
- new deb deps: x11-utils (Depends), chromium (Suggests)


## 0.16.0 (2026-07-07)

### Features

- pairing UI + relay slow tick (android/app)
- real bunker:// pairing + relay clause-poll + STATUS heartbeat (android/jni)
- Android device kind, hardened pairing, live last-seen (port-spec §5.2, §5.3) (charter-app)

### Bug Fixes

- first live relay round — rustls provider, INTERNET permission, poll diagnostics (android)
- percent-decode bunker-URI relay values on device+CLI; PWA emits literal wss:// (port-spec §5.1) (pairing)



## 0.15.0 (2026-07-06)

### Features

- Device Owner enforcer — real suspension + lock + install-lockdown (android/app)
- real warden core over shared crates (android/jni)
- Device Owner app skeleton + Rust JNI bridge, emulator-proven (android)

### Bug Fixes

- close review fail-opens — install-lockdown, staged modes, lock safety (android)



## 0.14.2 (2026-07-06)

### Bug Fixes

- never fake-approve a device ask offline; bound wrap freshness (charter-app)



## 0.14.1 (2026-07-05)

### Bug Fixes

- compute grant exp in the locked dimension's tz, never the phone's (charter-app)



## 0.14.0 (2026-07-05)

### Features

- ingest device asks; approve/deny publishes a signed GRANT (charter-app)
- 'Ask for more time' primary on the lock panel (charter-lock,charterd)



## 0.13.0 (2026-07-05)

### Features

- the child's schedule on the lock panel; fix next-open overcount (charterd,charter-lock)
- dialog-card UI over the dimmed desktop (charter-lock)
- alpha admin escape chord (Ctrl+Alt+Shift+Q) (charter-lock)
- real text rendering — software canvas + vendored TTF (charter-lock)
- deliver 10-/1-minute warnings to the child via notify-send (charterd)
- pass the locked child's uid to the lock (Log out target) (charterd)
- interactive lock — Log out / Shut down / Suspend buttons (charter-lock)
- pure UI model — action buttons + click hit-testing (charter-lock)

### Bug Fixes

- capture the desktop before mapping the lock window (charter-lock)
- correct lock copy off-edge; ~2s enforcement tick (charterd)
- survive a wedged logind; thaw before terminate (charterd,charter-lock)
- resolve the active display via VTNr / the kernel's active VT (charterd)
- resolve the active session's display via its TTY -> the Xorg serving that VT (charterd)
- IncludeInferiors overlay draws + journal forensics (charter-lock)
- draw via the composite overlay so the panel survives a frozen compositor (charter-lock)
- resolve the lock's XAUTHORITY from the display's Xorg -auth cookie (charterd)
- non-blocking action exec (panel stays live vs a frozen session) (charter-lock)
- reap the spawned notify-send (no zombie leak in the daemon) (charterd)



## 0.12.3 (2026-07-03)

### Bug Fixes

- scope root SDK vitest to its own tests (exclude apps/**) (ci)



## 0.12.2 (2026-07-03)

### Bug Fixes

- pin @vitest/coverage-v8 to match vitest 2.1.x (coverage provider crashed on 4.x mismatch) (ci)



## 0.12.1 (2026-07-03)

### Bug Fixes

- reconcile child freeze level-triggered (edge-freeze lost if child logs in after lock) (charterd)



## 0.12.0 (2026-07-03)

### Features

- set up the guardian on this phone + pair-this-laptop screen (mycharter)
- wire the local signer into the store (select + enable + reconnect) (mycharter)
- createLocalSigner — local-key guardian signer (self-sign) (mycharter)
- guardian bunker:// pairing-URI + npub builders (mycharter)
- persisted local guardian key store (get-or-create/clear) (mycharter)

### Bug Fixes

- local signer honors the in-app confirm gate + persists autoSign (mycharter)
- harden pairing-copy fallback + disable copy button while busy (mycharter)
- enableLocalSigner failure-rollback + mount-reconnect state correction (mycharter)



## 0.11.0 (2026-07-02)

### Features

- STATUS relay subscription (unwrap -> onStatus) (mycharter)
- consume the device STATUS feed (parse + unwrap + merge) (mycharter)
- emit the per-child STATUS feed to the guardian (kind 31114) (charterd)
- STATUS build glue + gift-wrapped emit_status (charterd)
- CHARTER_DEVICE_STATUS=31114 + StatusPayload wire type (proto)
- CHARTER_ENFORCE observe/freeze-only/enforce mode (charterd)

### Bug Fixes

- blank field keeps the current limit (no retype) (charter-settings)
- don't fail-closed the browser on an unconfigured host (charterd)
- resolve the locked child's real display + Xauthority (charterd)
- freeze the child's whole user slice (Cinnamon/Mint fix) (charterd)
- freeze the full systemd cgroup-v2 app-slice path (charterd)



## 0.10.0 (2026-07-01)

### Features

- IA pass — Home onboards then glances, Family manages, in-app guide (charter-app)



## 0.9.0 (2026-07-01)

### Features

- honest early-companion reframe (computer-first) (charter-app)



## 0.8.0 (2026-07-01)

### Features

- serve the setup guide as /setup.txt (nginx-static-friendly) (charter-app)



## 0.7.0 (2026-07-01)

### Features

- add /setup guide (human + AI-readable) (charter-app)

### Bug Fixes

- onboarding buttons scroll to their setup section (charter-app)



## 0.6.0 (2026-07-01)

### Features

- production starts clean — no demo family (charter-app)



## 0.5.0 (2026-07-01)

### Features

- graphical guardian pairing (bunker:// paste -> pin + bind) (charterd)
- lock-out recovery — never-lock-parent guard + admin recovery + thaw fail-safe (charterd)
- on-screen device pairing code (GUI + QR) (charterd)
- emit the device pairing code at boot (charterd)
- real device pairing — parse + store the device pubkey (mycharter)
- wire the real Signet signer + bunker:// pairing UI (mycharter)
- child resolver (store Child -> signer ChildTarget) (mycharter)
- RealSigner + Signet (NIP-46) transport — the signing core (mycharter)
- signed-inner NIP-59 gift-wrap of a CLAUSE (real crypto) (mycharter)
- Policy->CLAUSE wire mapping (remote-signing foundation) (mycharter)
- runtime precedence flip — Signet-first enforcement (charterd)
- ChildConfig — optional guardian subject binding + setup write (charterd)
- MultiChildEnforcer consumes EffectivePolicy (Signet-first) (charterd)
- child_policy::resolve_effective — Signet-first precedence (charterd)
- broker routes per-child clauses by subject (charterd)
- ClausePayload.subject — per-child clause targeting (charter-proto)
- ChildClauseStore — per-(subject,kind) authenticated clause store (charter-sys)
- multi-child wiring — per-child loop, settings picker, setup seeding
- multi-child enforcer — independent per-child times (charterd)
- graphical "Charter Setup" wizard — no terminal needed (setup)
- normie GUI to set device-only screen-time limits (charter-settings)
- device-only limits — standalone parental controls, no guardian (charterd)
- the lock screen shows why (schedule vs daily limit) (enforce)
- lock-display host config + parent install runbook (packaging)
- on-display lock screen + real Vt/Session control (charter-lock)
- wire the live CLI — real exec probe + pairing view + dispatch (charter-cli)
- live org.forgesworn.Charter1 zbus server + signal bridge (charterd)
- wire the real subscribe->verify->enact->enforce loop (charterd)
- real zbus client proxy for org.forgesworn.Charter1 (charter-ipc)
- real nostr relay transport (rustls-backed wss) (charter-sys)
- real shell-out/sysfs enactor ports (cgroup/flatpak/fapolicyd) (charter-sys)
- real OS-query ports (mount/account/polkit/systemctl) (charter-sys)
- real machine signer — load/generate key + schnorr sign (charter-sys)
- file-backed WebPolicy/Dns/ApprovedExec enactors (Step 2, verified) (charter-sys)
- real on-disk persistence (Step 1, verified) (charter-sys)
- Phase 9 packaging — installable .deb + lockdown assets (charter-linux)
- web enforcer feeds cached curator lists into the evaluator (charterd)
- refresh_curator_lists ingest driver, wired into poll_once (charterd)
- TransportFacade::poll_curator_lists + MockTransport scripting (charterd)
- CuratorListStore port (rollback-protected cache) wired into SystemLayer (sys)
- poll_curator_lists — fetch, verify (author+sig), dedup replaceable (transport)
- curator web-list wire parser (kind 30100 -> CuratorList) (transport)
- serde on CuratorList/CuratorEntry/Rating for cache round-trip (content)
- curator web-list tag vocabulary (namespace + rating tokens) (primitives)
- web content enforcer — load clause, evaluate, render, materialize via ports (change-detected, fail-closed, audit tags) (charterd)
- WebPolicyOps + DnsFilterOps ports (mock recorders + compile-only real) (sys)
- DNS-filter renderer — modes, forced SafeSearch + YouTube-restrict CNAMEs (webpolicy)
- charter-webpolicy crate — Firefox policies.json renderer (allowlist/blocklist/locked + hardening) (webpolicy)
- surface parentAllow as allow_exceptions for the DNS layer (content)
- add CHARTER_CURATOR_WEB_LIST kind (30100, provisional) (primitives)
- add ClauseKind::Content variant (store_key 3) (proto)
- blocklist posture — curator blocks + categories + parent override (content)
- allowlist posture — curator quorum + parent override (content)
- EffectiveWebPolicy + evaluator core with fail-closed paths (content)
- domain normalization helper (content)
- charter-content crate scaffold + GrantContent clause type (content)
- Stage 0/1 — device pairing, ask-to-install, onboarding (charter-app)
- Charter family-management PWA (parent app) (charter-app)
- Phase 8 gui-cli (autonomous core) — CLI + web UI (linux)
- Phase 7 time-extend-enactor — today-only additive extension (linux)
- Phase 6 schedule-budget-enforcer — time engine + min(window,quota) (linux)
- Phase 5 exec-gate-approved-store — hash-not-path exec broker (linux)
- Phase 4 install-flatpak-enactor — first brokered op end-to-end (linux)
- Phase 3 charterd-skeleton — fail-closed broker spine (mock transport) (linux)
- Phase 2 transport-pairing-audit — NIP-44/59 + bunker:// + relay (linux)
- Phase 1 grant-verify-core — proto + verify on real nostr vectors (linux)
- Phase 0 workspace-foundation — shared crates, sys layer, DOB guard (linux)

### Bug Fixes

- remediate pre-test review findings (device + PWA) (charter)
- remediate adversarial-review findings on the 3 normie features (charterd)
- remediate adversarial-review findings (2 fail-opens + test gaps) (charterd)
- polish — lead Home with child status, compact onboarding, PWA cleanups (charter-app)
- paused beats revoked — fail-closed precedence per spec §4.3 (content)
- address review findings — strip DNS root dot so parentDeny can't be bypassed (content)
- remediate review findings across the warden core (linux)



## 0.4.0 (2026-06-28)

### Features

- ship MyCharter + guide to main for charter.mysignet.app (charter-app)



## 0.3.0 (2026-05-25)

### Features

- initial consumer SDK implementation (Charter Phase 1α)



## 0.2.0 (2026-05-25)

### Features

- initial consumer SDK implementation (Charter Phase 1α)


