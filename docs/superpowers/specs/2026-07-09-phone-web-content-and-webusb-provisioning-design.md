# Phone web-content enforcement + WebUSB provisioning — design

**Date:** 2026-07-09
**Status:** approved (decented, 2026-07-09)
**Batch:** the "normie full stack" bold batch — two features, one hardware round.

## Goal

Close the two gaps that keep a non-technical guardian out of the phone stack:

1. **The phone honors the web-content clause.** A guardian can already author
   SafeSearch / YouTube-restrict / domain allow-block in MyCharter, and the phone
   already receives, verifies, and persists that clause — but enforces none of it
   (`apps/charter-app/src/domain/types.ts:62` — "stored-but-inert"). After this batch
   the same clause binds on the phone via an on-device DNS filter.
2. **Onboarding without a terminal.** Device Owner setup today is a raw
   `adb shell dpm set-device-owner` (QR/NFC/zero-touch provisioning is dead on stock
   GrapheneOS — founder-confirmed; the upstream SetupWizard2 PR #40 is open but
   unmerged as of July 2026). After this batch, MyCharter itself provisions the phone
   over WebUSB from a Chromium browser: plug in a cable, click through, phone comes up
   owned **and paired** — zero scans, zero terminal.

## Non-goals

- Full-traffic TUN forwarding (NetGuard-style) to catch apps with hardcoded DNS
  servers. DNS-only interception matches the declared cooperative/anti-casual threat
  tier (`graphene/concept.md`); full-TUN is the named hardening follow-up.
- Charter-app self-update on the phone (new versions install via a WebUSB re-run or a
  later DO self-update path).
- QR managed provisioning (blocked upstream; add the standard payload path if/when
  GrapheneOS merges SetupWizard2 PR #40).
- Curated-allowlist authoring UX in the PWA (v1 keeps `curators` empty; the phone
  gains the *ingest* plumbing only, for evaluator parity with Linux).

## Decided forks

| Fork | Decision | Why |
|---|---|---|
| Filter failure posture | **Fail-closed**: always-on VPN with lockdown — service death blocks data until it self-heals | Matches the fail-closed spine; a crash must never be a discoverable filter bypass. Calls/SMS unaffected (data only). |
| Enforcement mechanism | **On-device DNS-filtering VpnService** reusing the Linux `DnsFilterPlan` | No server infra, offline-safe, covers every app using OS DNS, single evaluator + single renderer across stacks. Hosted-resolver (privacy, infra, offline-fail) and managed-browser-config (Vanadium coverage, browser-only) rejected. |
| Onboarding path | **WebUSB browser flow in MyCharter** (ya-webadb), shell script as fallback | Same UX family as the GrapheneOS web installer the parent already used; kills the terminal entirely. |

## Part 1 — Phone web-content enforcement

### Architecture

One evaluator, one renderer, two enactors. Rust decides and renders the plan; Kotlin
only enacts it. This extends the existing decide/enact split (port-spec §3.5).

```
content clause (kind 3, already on phone)
  → evaluate_content_json          (charter-content, shared, fail-closed)
  → render_dns_filter → DnsFilterPlan   (charter-webpolicy, shared)
  → charterDnsPlan() JNI export          (new, follows charterAppPolicy precedent)
  → Kotlin DnsFilterOps / CharterVpnService  (new, enact-only)
```

### Rust / core changes

- **Move `charter-webpolicy` from `linux/crates/` to `core/crates/`.** It is a pure
  no-I/O renderer pair (Firefox policies + DNS plan) and belongs in core by the
  repo's own split; Linux crates keep depending on it unchanged (same extraction
  pattern as charter-spine, port-spec §1.3).
- **Content branch in the Android resolver** (`android/jni/src/warden.rs`,
  `resolve_effective` ~L1106): load stored clause kind 3 → `evaluate_content_json`
  (undecodable ⇒ Locked, fail-closed) → `render_dns_filter` → cache plan JSON plus a
  revision hash for idempotent apply.
- **New JNI export `charterDnsPlan()`** (`CharterNative.kt` / `CharterCore.kt` typed
  wrapper), mirroring `charterAppPolicy()` — returns `{revision, plan}`.
- **Curator-list ingest on the slow poll**: call the existing `poll_curator_lists`
  (`android/jni/src/relay.rs:311`) and store via the existing
  `RealCuratorListStore` (`android/jni/src/system.rs:262`) so allowlist-with-quorum
  evaluates identically to Linux. Inert until a parent subscribes curators.
- **Firefox DoH canary in the shared renderer**: `use-application-dns.net` ⇒ NXDOMAIN
  added to `render_dns_filter` if not already present, so both stacks emit it from
  one place.

### Kotlin changes (enact half)

- **`DnsFilterOps` capability interface** in `enforce/Enforcement.kt` style
  (mirrors `AppGateOps`): `apply(plan)` / `clear()` / `isActive()`, injectable fake
  for tests.
- **`CharterVpnService : VpnService`**: TUN whose only routed destinations are the
  virtual DNS servers (IPv4 + IPv6 ULA, so v6-only networks neither bypass nor
  break). In-service resolver:
  - blocked domain (or Locked mode, minus allow-exceptions) ⇒ NXDOMAIN;
  - SafeSearch/YouTube names ⇒ the plan's CNAME rewrites
    (`forcesafesearch.google.com`, `restrict[moderate].youtube.com`,
    `strict.bing.com`, `safe.duckduckgo.com`) — resolve the rewrite target upstream,
    answer under the queried name;
  - everything else relayed to the underlying network's real resolvers through
    `protect()`ed sockets — UDP relay plus a minimal TCP:53 relay.
- **DO pinning** in `WardenController.init` (beside `setLockTaskPackages`):
  `setAlwaysOnVpnPackage(admin, self, lockdown = true, lockdownAllowlist =
  [com.android.captiveportallogin])` so captive-portal Wi-Fi sign-in still works.
  Baseline restrictions (`DpmRestrictionOps.baseline`) gain `DISALLOW_CONFIG_VPN`
  and `DISALLOW_CONFIG_PRIVATE_DNS`.
- **Gating** matches the app gate: active when configured and mode ≠ Observe, so the
  staged bring-up (observe → suspend-only → enforce, hardening #22) is intact. Once
  up, the VPN stays up even under an Unrestricted plan (pass-through) — policy flips
  are instant and the network stack never churns.
- **Wire-in**: `WardenController` ctor + `real()` factory + `applyDecision` beside
  `appGate.reconcile`, idempotent on plan revision. Manifest gains the
  `BIND_VPN_SERVICE` service declaration. The VPN package joins `denyListPackages`
  so the app gate can never suspend it.

### PWA + contract

- **Zero wire changes.** MyCharter already publishes the content clause to phones.
- Copy: `Limits.tsx` "(Enforced on computers today.)" → computers **and phones**;
  update the `WebPolicy` doc comment in `domain/types.ts`.
- `spec/contract.md`: flip `content` from *reserved* to specified (shape =
  `GrantContent`, `core/crates/charter-content/src/clause.rs:33`). The pending
  wardship-lexicon rollout lands in the same editing pass.

### Failure handling

| Failure | Behavior |
|---|---|
| VpnService death | OS lockdown blocks data until `START_STICKY` + BootReceiver revive it (fail-closed by decision). |
| Enforce mode entered, no plan yet | VPN up, resolver pass-through. |
| Malformed/undecodable clause | Locked plan — block-all with exceptions (same as Linux). |
| Charter's own relay traffic | Bypasses the TUN via protected sockets; enforcement can never strangle the guardian channel. |
| Captive portals | Lockdown-exempt portal-login app. |

Accepted, documented limits (anti-casual tier): hardcoded-DNS apps bypass DNS-only
filtering; DoH browsers are handled by canary + `DISALLOW_CONFIG_PRIVATE_DNS`, not
packet inspection. Audit stays classification-only — no URLs — matching Linux.

## Part 2 — "Set up a phone" (WebUSB provisioning)

### The flow

New MyCharter wizard screen (Chromium-only; graceful "use Chrome/Edge/Brave" notice
elsewhere). Library: `@yume-chan/adb` + `@yume-chan/adb-daemon-webusb` (ya-webadb,
MIT).

1. **Prep** — illustrated on-phone steps: finish the GrapheneOS wizard with **no
   accounts**; enable Developer options (Build number 7×); enable USB debugging;
   plug in the cable.
2. **Connect** — WebUSB picker; adb handshake; parent accepts the one RSA-trust
   dialog on the phone.
3. **Preflight** — over adb: no accounts / no extra users (the `dpm` precondition),
   GrapheneOS build check, battery sane. Each failure gets specific remediation text.
4. **Install + own** — fetch the release Charter APK same-origin (version-pinned
   filename, SHA-256 displayed), stream `pm install`, then
   `dpm set-device-owner org.forgesworn.charter/.admin.CharterDeviceAdminReceiver`,
   verify output.
5. **Pair over the cable — zero scans.** The PWA mints the same `bunker://` URI it
   puts in today's pairing QR and fires it at the phone
   (`am start -a android.intent.action.VIEW -d '<uri>'`) into MainActivity's
   existing `handlePairingIntent`; then watches its own relay intake until the
   phone's STATUS heartbeat appears. The success screen is the phone going live in
   MyCharter. The QR path remains for re-pairing.
6. **Fallback** — every step has a "do it manually" expander with the exact adb
   command; `android/scripts/charter-provision.sh` ships as the scripted fallback
   (making the port-spec §3.8 reference true).

### Release APK + signing

- The flow requires a properly signed **release** build: no `testOnly` (that is
  debug-manifest-only today), stable signing cert, hosted in the PWA deploy (push to
  main → pipeline serves it).
- The release keystore is a new one-time artifact: generate, document, and record the
  cert SHA-256 (no fabricated values — the displayed hash comes from the real
  artifact). Parent-gated RELEASE unpair remains the undo path for a DO'd phone.

## Testing

- **Rust:** golden `DnsFilterPlan` vectors pinned cross-stack (same vector files as
  Linux — the §5.6 parity precedent); JNI-path tests for the Content branch and
  `charterDnsPlan()`; curator-ingest test against the mock relay.
- **Kotlin:** pure-JVM unit tests (new `src/test`) for the DNS message codec and
  decision path; instrumented emulator tests on the existing DO harness: blocked
  domain ⇒ NXDOMAIN, SafeSearch CNAME answer, plan-flip idempotence, service-kill ⇒
  lockdown.
- **Provisioning:** ya-webadb speaks adb-over-TCP from Node — the exact
  install → own → pair command sequence gets a headless CI integration test against
  the emulator. The WebUSB picker/cable itself is untestable headlessly; that is
  what the hardware round covers.
- **Emulator rounds run autonomously** (existing hw-test harness recipe) before the
  hardware gate.

## The single hardware gate (decented)

The normie unboxing, on the reset Pixel: plug into the laptop → MyCharter "Set up a
phone" → click through → phone appears live in MyCharter (no terminal, no scans) →
author a web policy → on the phone: Google search is forced-SafeSearch, a blocked
domain dies, YouTube is restricted → reboot: everything persists; brief
service-kill check shows data blocked, then self-heal.

## Two clocks

- **Code-ready:** ~two focused build-sessions (content warden first, then
  provisioning).
- **Hardware-verified & shipped:** after the one gate above.

## Risks / watch-list

- Split-tunnel routes under always-on + lockdown: verify on emulator early that
  non-DNS traffic flows normally while only the DNS /32-/128 routes enter the TUN.
- Vanadium DNS behavior: expected to use OS DNS (no default DoH); verify on-metal.
- WebUSB on the parent's desktop Linux may need the same udev situation as the
  GrapheneOS web installer; document.
- `dpm set-device-owner` refusals (account races, already-provisioned) must surface
  with fixes, not fail opaquely.
- Upstream watch: GrapheneOS SetupWizard2 PR #40 — when merged, add the standard QR
  provisioning payload as a second path.
