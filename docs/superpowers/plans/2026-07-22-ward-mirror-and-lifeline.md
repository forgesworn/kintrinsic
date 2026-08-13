# Ward Mirror (D8) + Communication Lifeline (D9) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The ward always knows where they stand (time-left + schedule on the ward device, incl. a home-screen widget), and a locked phone can always call its guardian (lock-screen "Call your guardian" from a guardian-authored lifeline clause).

**Architecture:** Follow the established clause recipe end-to-end (the `tethering`/`update` clauses are the precedents): new `ClauseKind::Lifeline` + body in charter-proto → stored via the existing rollback-protected clause store → read-surface on the warden → JNI → Kotlin. D8 needs NO new Rust: `charterTimeLeft` + `charterLockInfo` already expose minutes-left and the rendered weekly schedule lines. Calls use `ACTION_CALL` + DO-self-granted `CALL_PHONE` (mirrors the POST_NOTIFICATIONS self-grant), avoiding putting the dialer app inside the LockTask allowlist.

**Tech Stack:** Rust (charter-proto, charter-spine, android/jni), Kotlin (ward app :app), React/TS (MyCharter PWA).

## Global Constraints

Same as the carrier plan: `source ~/Android/env.sh`; Android not in CI (verify locally); JNI off the main thread; no `mock` in release .so; wardship lexicon in copy; PWA `npm test` stays green; core gates = `cargo test` in each touched crate + `cargo test` in `android/jni`; commit per task.

**Precedents to mirror (read before each task):**
- Clause kind + body: `core/crates/charter-proto/src/clause.rs` (`ClauseKind::Update`, `UpdateAppBody`) — Lifeline gets `store_key` **9**.
- Warden read-surface + JNI: `android/jni/src/warden.rs:461` (`tethering_mode`) and `android/jni/src/lib.rs` (`charterTetheringMode`).
- DO permission self-grant: `WardenController.kt:79-84` (POST_NOTIFICATIONS).
- Lock-screen button + worker: `LockActivity.kt` ask-for-time path.
- PWA clause mapping: `apps/charter-app/src/wire/clause.ts` (`tetheringToGrant` + `policyToClauses` base()).

### Task 1: `ClauseKind::Lifeline` + `LifelineBody` (charter-proto)
**Files:** Modify `core/crates/charter-proto/src/clause.rs` (+ re-export in `lib.rs`).
`LifelineBody { v: u32, numbers: Vec<LifelineNumber>, issued_at: u64 }`, `LifelineNumber { label: String, number: String }` — camelCase serde, strict `from_json` (v==1, 1..=3 numbers, non-empty label ≤ 20 chars, number matches `^[+0-9][0-9 ()-]{4,19}$`). Tests: roundtrip, empty-numbers reject, bad-number reject, v2 reject. Gate: `cargo test -p charter-proto`.

### Task 2: Spine store + warden read-surface + JNI
**Files:** Modify spine clause ingest/store to accept `lifeline` (follow wherever `tethering`/`update` kinds are validated + persisted), add `Warden::lifeline()` in `android/jni/src/warden.rs` returning the current `LifelineBody` as JSON (or "" unpaired/none), add `charterLifeline()` to `android/jni/src/lib.rs` + `CharterNative.kt`. Tests: warden test mirroring `tethering_mode` tests (ingest lifeline clause → read numbers; revoked/absent → ""). Gate: `cargo test` in `core` crates touched + `android/jni`.

### Task 3: Lock-screen "Call your guardian" (ward Kotlin)
**Files:** Modify `LockActivity.kt` (buttons from `charterLifeline()`, worker-thread read, `ACTION_CALL` tel: with the number), `WardenController.kt` (self-grant `CALL_PHONE` beside POST_NOTIFICATIONS), `android/app/src/main/AndroidManifest.xml` (`<uses-permission android:name="android.permission.CALL_PHONE"/>`).
Copy: "Call {label}" (e.g. "Call Mum"). Buttons render only when numbers exist. NOTE in code: InCall-UI-over-LockTask is a hardware-verify item; fallback documented (add dialer to `setLockTaskPackages`). Gate: `:app:assembleDebug` + existing `:app` unit tests stay green.

### Task 4: PWA lifeline authoring
**Files:** Modify `apps/charter-app/src/domain/types.ts` (Policy.lifeline), `src/wire/clause.ts` (`lifelineToGrant` + emit in `policyToClauses`, mirror tethering), `src/screens/Family.tsx` (per-child "Guardian phone numbers" editor, 1–3 entries, label+number, save → setSchedule-path publish), tests in `src/wire/updateClauseVectors.test.ts` sibling style + a Family interaction test if the screen has one. Gate: `npm test` + `npm run build`.

### Task 5: Ward status block — "Your charter" (D8 screen)
**Files:** Modify ward `MainActivity.kt`: when paired, show a status section — minutes left today (from `charterTimeLeft`) + the schedule lines (from `charterLockInfo`.lines) — read on the existing UI worker, refreshed onResume. Read-only, ward-warm copy. Gate: `:app:assembleDebug`.

### Task 6: Time-left home-screen widget (D8 widget)
**Files:** Create `android/app/src/main/kotlin/org/forgesworn/charter/ui/TimeLeftWidget.kt` (AppWidgetProvider), `res/layout/widget_time_left.xml`, `res/xml/widget_time_left_info.xml`, manifest receiver. Updates: `CharterService` tick pushes `charterTimeLeft` minutes into the widget (level-triggered on change), plus standard `onUpdate`. Display: big minutes number + "left today" / "locked" / "no charter yet". Gate: `:app:assembleDebug`, emulator smoke (place widget, verify text).

### Task 7: Gates + evidence + merge
All cargo tests, `:app` + `:carrier` unit tests, both APK assembles, PWA tests/build. Evidence notes appended to `docs/superpowers/specs/2026-07-22-carrier-emulator-round.md` sibling file or a new D8/D9 round log. Hardware-round items recorded: InCall-over-LockTask, emergency affordance under LockTask, incoming-call-over-lock, widget on real launcher.
