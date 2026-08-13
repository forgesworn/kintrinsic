# Ward Mirror (D8) + Lifeline (D9) — Build Evidence

**Date:** 2026-07-22 (same-day follow-on from the carrier APK).

## What shipped

**D9 — communication lifeline:**
- `ClauseKind::Lifeline` (store_key 9) + fail-closed `LifelineBody` (1–3 numbers,
  strict dial-string charset — no USSD/`#` smuggle) in charter-proto.
- Warden `lifeline()` read-surface + `charterLifeline` JNI; malformed body ⇒
  NO buttons (proven by test: a superseding `*#06#` body renders nothing).
- Lock screen: "📞 Call {label}" buttons, `ACTION_CALL` direct-dial with
  DO-self-granted `CALL_PHONE` — the dialer app never joins the LockTask
  allowlist. Failure copy is honest, never silent.
- MyCharter: Lifeline section in Limits (up to 3 label+number rows, client-side
  mirror of the device validator), `lifeline` clause on the wire, store plumbing.

**D8 — ward mirror:**
- Warden `schedule_view()` + `charterScheduleView` JNI — lock state, minutes
  left, and the week lines rendered by the SHARED spine composer
  (`charter_spine::lock_info`), so the phone and the Linux lock panel phrase
  the week identically.
- Ward app main screen: "Your charter" block (paired only) — time left /
  locked-now + the weekly schedule. "No charter set yet — ask your guardian"
  beats a silent blank.
- Home-screen time-left widget (`TimeLeftWidget`), fed level-triggered from the
  CharterService tick; JNI kept off the main thread in launcher callbacks.

## Gates (all green)

- linux workspace: 167 passed / 0 failed (new clause kind is accept-and-ignore
  on the Linux warden — forward-compatible)
- core workspace: 329 passed (incl. 3 new proto lifeline tests)
- android/jni: 25 passed (incl. `lifeline_clause_exposes_numbers_fail_closed`,
  `schedule_view_mirrors_the_week`)
- gradle `:app` + `:carrier` unit tests + assembles: BUILD SUCCESSFUL
- PWA: 36 files / 211 tests + 2 new wire tests (17 in clause.test.ts) + build

## NOT yet proven (hardware-round items, folded into the standing round)

- The call button on a REAL locked phone: does the in-call UI show over
  LockTask? (Fallback ready: add the dialer to `setLockTaskPackages`.)
- Emergency dialing affordance under LockTask (the code comment's claim,
  never verified on metal).
- Incoming calls over the lock surface.
- Widget on a real launcher; mirror screen against a real schedule.
- OEM dialer package names beyond AOSP/GrapheneOS (parked until the fleet
  widens beyond Pixels).
