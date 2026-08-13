# Always available apps — follow-ups and rulings

**Date:** 2026-08-04
**Branch:** `feat/always-available-apps` (15 build commits + 6 fix commits)
**Spec:** [2026-08-03-always-available-apps-design.md](2026-08-03-always-available-apps-design.md)

What the build turned up and deliberately did not fix, plus the calls that were
made along the way. Written down because the review workspace is scratch and
this is the part worth keeping.

## Rulings (decided, not open)

**Dormant devices honour always-available apps.** A device set to "off unless I
open it" reports `LockReason::Schedule` — a routing convenience so gift/ask keeps
working — and `exempt_packages` matches that string, so named apps open there.
decented ruled this correct: *"always available means always"*; the naming **is**
the standing exception, and dormant means off-by-default, not off-absolutely. A
stand-down stays excluded because that is an active intervention. `contract.md`
was rewritten accordingly — the old justification ("the locks a device reaches on
its own clock") never described dormant.

**The clause is category-neutral.** It is not an audiobook feature. decented:
*"we don't know what that app is… it could be a weather app. It could be a
specific game that helps reduce anxiety. It could be a messaging app."* Keep
every doc comment and every piece of copy neutral; examples are fine, a
definition in terms of audio is not.

**Multi-device night counts use max, not sum.** A guardian with two Android
devices used out-of-hours on two nights each sees 2, not 4. Summing would
double-count a shared night, and STATUS carries no per-day detail to union
correctly. Least-bad at this wire shape — see the follow-up below.

## Follow-ups, roughly by value

**Per-day out-of-hours detail on the wire.** Closes the max-of-nights
understatement above. Worth doing before two-Android families are common;
understating a ward's use to her guardian is the wrong direction for an error to
point.

**Scope the neighbouring line, not just ours.** `weekSummary` says "This week:"
about a **rolling last-7-days** window; the out-of-hours line says "so far this
week" about a **calendar** week. The rewording stops ours claiming a window it
does not measure, but two lines an inch apart still both say "this week".
Renaming the neighbour to "Last 7 days:" finishes it, and needs no cross-language
fixture change — that line is TypeScript-only.

**`paintOpenRow` makes four JNI reads on the main thread.** `render()` documents
"never the main thread (port-spec §2.3/§3.2)". Pre-existing and dwarfed by the
`PackageManager` scan already there, but the shade is where jank shows most.

**`DpmRestrictionOps.applyTetherMode` has no `runCatching`.** A throw kills the
whole tick — no lock re-assert, no suspend reconcile — until the next iteration.
Pre-existing, genuinely out of this plan's scope, and a fail-open in the tick
loop. `syncOnChange` now exists to absorb it.

**Copy: the ward cannot turn a dormant device on.** `Limits.tsx` says "or set to
stay off until you or she turns it on". She asks; the guardian approves. The
canonical phrasing elsewhere is "Off — you open it when needed".

**`MainActivity.kt:427`** still carries the absolute "there is no guardian-only
variant of this sentence" that was softened everywhere else — the shared fixture
pins the *formatter*, while the *inputs* are per-device.

**Android's `dayKey` tz precedence differs from the guard's.** The guard uses
`schedule.tz ?? budget.tz ?? UTC` (a rule written about charterd); Android stamps
from `budget.tz → buckets.tz → enforcement_tz`. They diverge only for mixed-tz
configs with no budget, cost one boundary day, and fail silent (the line
disappears; the guardian never sees a number the ward cannot). Wants a comment in
`outOfHours.ts` so the next reader does not assume the Linux rule applies.

**`currentWeekDayKeys` adds exact 86400s multiples**, so a mid-week DST move
could shift a day key by one. In common zones the transition sits at the week
edge and the error lands on a future day — harmless today, real if a zone ever
moves mid-week.

**Untested boundary:** no test drives `weekStart: "sun"` through
`outOfHoursContributions` (only through `outOfHoursGuardWeekStart`). Correct by
inspection; a Sunday-`dayKey`-under-Sunday-start case would pin it.

**Make `openRowEntries`' three new params required.** One production call site
today, all three passed. The defaulted signature is a trap if a second appears.

**`appHold` / `cmdlineIdentity` still hand-roll their too-old sentences** rather
than using the new shared `tooOldNote()`.

## Release gates (not merge gates)

- `apps/charter-app/public/charter-apk.json` still reads versionCode 38 /
  0.6.7. The parity gate is `android: 39`, so **every ward reads as too-old
  until a signed release ships** — correct, fail-safe, and the guardian is told
  plainly rather than watching a setting do nothing.
- After any release: `scripts/sync-front-door-downloads.sh` + push `charter-you`,
  or charter.signet.you keeps serving the old build.
- Verify the release APK carries no mock seams (~27 MB, zero `wire_test_relay`
  strings) before publishing — `android/scripts/build-jni.sh` gates it.
