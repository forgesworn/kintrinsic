# Lifeline v2 + Break-Glass Override — Implementation Plan (Opus-executable)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Grow the lifeline to 5 numbers + a region-correct emergency entry with anti-pocket-dial friction, and add the break-glass override (instant, offline-capable, loudly transparent unlock).

**Authority documents (READ FIRST, follow exactly — all decisions are made):**
- Design: `docs/superpowers/specs/2026-07-24-lifeline-incall-and-break-glass.md`
- Wire (frozen): `spec/contract.md` §"Break-glass override event" + device audit
- Values: `docs/CONSTITUTION.md` (no rate limits on the override; transparency is the mechanism, warm copy)

**Already DONE (0.3.1, commit 86bc8a8) — do not redo:** in-call passthrough (dialer in LockTask allowlist), End-call bar on the shade, READ_PHONE_STATE/ANSWER_PHONE_CALLS self-grants.

**Tech Stack:** Rust (core/crates, android/jni), Kotlin (android/app), TypeScript (apps/charter-app). Vector pattern: JSON under `core/crates/charter-testkit/vectors/`, asserted from Rust tests AND `apps/charter-app/src/wire/*Vectors.test.ts`.

## Global Constraints
- **Ship order is load-bearing:** device-side ACCEPTANCE of the wider lifeline shapes lands and is RELEASED to devices (self-update) BEFORE MyCharter authors them. Fail-closed validation on old devices renders NO lifeline otherwise.
- Lifeline numbers: 1..=5 (was 1..=3). `emergencyServices?: boolean` — the device resolves the number via `TelephonyManager.getEmergencyNumberList()`; a digit string for it NEVER crosses the wire. Never fabricate any number/value (repo rule).
- Break-glass: NO rate limits, NO cooldowns, works offline (journal → audit on reconnect). Audit tags exactly as the contract pins them.
- Friction (ward shade, all call buttons): hold-to-call ~2s (filling indicator + haptic; early release cancels) THEN a 3-2-1 cancel window before dialing. Longer hold (~3.5s) for the emergency entry.
- Gates: `cd core && cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace`; same from `linux/`; `cd android/jni && cargo fmt && cargo clippy --all-targets && cargo test`; `cd android && ./gradlew :app:testDebugUnitTest assembleRelease`; `cd apps/charter-app && npx tsc --noEmit && npx vitest run && npm run build`. Android is NOT in CI — run locally, always.
- Commit per task with the session trailer convention visible in `git log`.

---

### Task 1: Rust — lifeline body v2 acceptance (device side first)

**Files:** `core/crates/charter-proto/src/clause.rs` (LifelineBody + its validation), its unit tests, `core/crates/charter-testkit/vectors/` lifeline vectors (find the existing lifeline vector file via `grep -rn lifeline core/crates/charter-testkit/vectors/`), the Rust consumer test, and `apps/charter-app/src/wire/` mirror test if one exists.

- [ ] Find `LifelineBody` and its 1..=3 bound; relax to 1..=5. Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub break_glass: Option<BreakGlassCfg>` and `pub emergency_services: Option<bool>` (camelCase wire: `breakGlass`, `emergencyServices`). `BreakGlassCfg { enabled: bool, scope: BreakGlassScope /* Calls|Full, lowercase wire */, duration_minutes: u16 }`.
- [ ] Tests: 5 numbers accepted; 6 rejected; old 3-number payloads byte-stable (existing vectors untouched and passing); new optional fields roundtrip camelCase and are omitted when None; unknown-field tolerance unchanged.
- [ ] Add NEW vectors (do not edit existing entries) for a 5-number + breakGlass + emergencyServices payload; assert from Rust and (if a TS lifeline vector test exists) TS.
- [ ] Run core gates; commit `feat(core): lifeline v2 wire — 5 numbers, emergencyServices flag, breakGlass config (acceptance first)`.

### Task 2: Android JNI — surface the new fields to Kotlin

**Files:** `android/jni/src/warden.rs` (the `lifeline()` DTO — grep `fn lifeline`), `android/jni/src/dto.rs` if the DTO lives there, `android/app/.../native/CharterCore.kt` + `CharterNative.kt` (the JNI mirror), plus the JNI glue in `android/jni/src/lib.rs`.

- [ ] Extend the lifeline DTO with `emergencyServices: bool` and a `breakGlass` sub-struct (enabled/scope/durationMinutes), sourced from the verified lifeline clause. Absent clause fields → disabled/false (fail-closed).
- [ ] Rust unit test: a v2 clause round-trips through the DTO; a v1 clause yields defaults.
- [ ] `cd android/jni && cargo test` + `cargo clippy`; Kotlin compiles. Commit.

### Task 3: Kotlin shade — 5 buttons, regional emergency, hold-to-call friction

**Files:** `android/app/src/main/kotlin/org/forgesworn/charter/ui/LockActivity.kt` (the lifeline block, ~line 231; the End-call bar and call poll already exist above it — reuse their patterns).

- [ ] Render up to 5 lifeline buttons (list already comes from `CharterCore.lifeline()`).
- [ ] When `emergencyServices` is on: append an entry labeled with the REAL local number — resolve via `TelephonyManager.getEmergencyNumberList()` (flatten, first CALL-category number; fallback `112`); label "🚨 Emergency <number>". Dial it with `ACTION_CALL` like the others (verify on hardware; if ACTION_CALL to an emergency number is rejected by telecom, fall back to `ACTION_DIAL` — note it in the commit).
- [ ] Friction, ALL call buttons: replace the click listener with an `OnTouchListener` hold-to-call — ACTION_DOWN starts a 2000ms (3500ms for emergency) countdown shown by updating the button text ("Keep holding… "), haptic at start and completion (`performHapticFeedback`); ACTION_UP/CANCEL before completion resets. On completion, a 3-2-1 cancel window: button text "Calling <label> in 3…" ticking per second, any tap cancels; at 0 the ACTION_CALL fires. Implement as a small self-contained state machine class `HoldToCall` in the same file with pure-logic timing driven by `askWorker.postDelayed` so it is unit-testable in isolation from views if a host test exists; otherwise keep it simple and hardware-verify.
- [ ] `./gradlew :app:testDebugUnitTest assembleRelease`; commit `feat(android): lifeline v2 shade — 5 numbers, regional emergency entry, hold-to-call + cancel-window friction`.

### Task 4: Break-glass — device side

**Files:** `android/jni/src/warden.rs` (+ a small journal file next to the outbox — follow `android/jni/src/outbox.rs` pattern), `android/jni/src/lib.rs` (JNI entry `charterBreakGlass`), `CharterNative.kt`/`CharterCore.kt`, `LockActivity.kt`.

- [ ] JNI/Rust: `break_glass(now) -> BreakGlassResult`: reads the verified lifeline clause's `breakGlass`; if disabled → `{allowed:false}`. If enabled: record `{at, scope, duration_secs}` durably (survives restart), return allowed+scope+duration. While a break-glass window is active, the warden's decision is UNLOCKED for that scope (`full` → not locked; `calls` → keep locked but the shade knows calls are open — the shade already lets calls through, so `calls` scope changes nothing in enforcement, only messaging). Window expiry → normal enforcement resumes (level-triggered, next tick).
- [ ] Audit emission: on activation, emit the device audit (kind 31000) with EXACTLY the contract's tags (`outcome=override`, `op=unlock.breakglass`, `scope`, `durationSecs`), content empty, via the same emit path as existing audits (grep `emit_audit` in warden/relay). Offline: park it in the outbox pattern; it emits on reconnect. The unlock NEVER waits for the emit.
- [ ] Rust tests: disabled → not allowed; enabled full-scope → unlocked for duration then relocks (drive ticks); audit event carries the exact tags; restart mid-window stays unlocked until expiry (durable).
- [ ] Shade UI: when enabled, a distinct bottom affordance "🚨 Emergency unlock — hold" with the SAME hold+cancel friction (3.5s) and a warm confirmation line stating the transparency plainly: "Your phone will unlock for N minutes. <Guardian label> will be told." Then it calls `charterBreakGlass` and the shade dismisses (full) or re-renders with "Phone unlocked for calls" (calls).
- [ ] Gates + commit.

### Task 5: Guardian side — MyCharter knobs, carrier alarm, Activity

**Files:** `apps/charter-app/src/screens/` (the Limits/child policy editor holding today's lifeline editor — grep `Lifeline` in src/), `src/wire/clause.ts` (lifeline clause builder), `src/store/store.tsx` (audit intake path — grep how `locked/thawed` audits or activity events are ingested), `src/screens/Activity.tsx`, `android/carrier/src/main/kotlin/org/forgesworn/mycharter/service/` (wrap classifier + `Notifier.kt`).

- [ ] MyCharter lifeline editor: allow up to 5 numbers; add the "Emergency services number (999/911 — set automatically by the phone)" toggle; add the Break-glass card (enable toggle, scope choice with plain-language copy, duration). Builder emits the v2 fields ONLY when set (old payload byte-stable otherwise — vector-guarded from Task 1).
- [ ] Carrier: classify inbound audit wraps with `outcome=override` (see `classify.rs` + its Kotlin consumer) → an URGENT notification, same channel as asks: "⚠️ <Ward> used the emergency unlock". Tap → MainActivity (pattern exists in Notifier.kt).
- [ ] MyCharter Activity: render override audits as a distinct entry ("Sam used the emergency unlock — phone open for 10 minutes"), warm not alarming; it also appears in the week naturally via usage.
- [ ] PWA gates; commit.

### Task 6: Release + ship-order enforcement

- [ ] Bump `android/app/build.gradle.kts` versionCode/Name; `./scripts/publish-apk.sh`; commit `apps/charter-app/public` + push (deploy serves it). decented sends the update from MyCharter.
- [ ] ONLY AFTER the phone(s) report the new versionCode in STATUS may the MyCharter authoring UI from Task 5 be released for use with >3 numbers — if Tasks land together in one push, add a guard in the lifeline editor: cap authoring at 3 unless every paired device's `appVersionCode` ≥ the new code (deviceStatus has it). Implement the guard; it self-lifts as devices update.
- [ ] Update the design memo status + `spec/contract.md` coordination note; final gates everywhere; push.

## Self-review checklist for the executor
- Ship-order guard actually reads `appVersionCode` from live `deviceStatus`, not from state guesses.
- No emergency digit strings on the wire anywhere; the device resolves locally.
- Break-glass has NO cap/cooldown code paths; only friction + transparency.
- All copy is wardship-lexicon, warm, and honest (see existing lock-screen strings).
