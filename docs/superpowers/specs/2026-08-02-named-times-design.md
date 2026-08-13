# Named times — one time model for the family

**Date:** 2026-08-02 · **Status:** approved direction (decented), building
**Decisions locked in brainstorming:** groups are shared family vocabulary (ward
sees the names) · one unified concept replacing learning-vs-buckets in the UI ·
suggestions-not-taxonomy on first contact · asks and gives speak group names this
round · weekly allowances included · on-request apps included.

## 1. The idea

Charter never decides what an app *is* — no store categories, no genre
taxonomy. Minecraft is a game, a social space, and a classroom at once; only the
family can say what it is *for them*. So the family names its own times.

The whole time model, as a parent explains it to a child:

> Your day has a total. Inside it, we've named some times together:
> **Free** times don't spend your day (Khan Academy). **Counted** times have
> their own allowance (Play: an hour a day, five hours a week). A few apps are
> **On request** — installed, but you ask before you open them. Everything else
> is just ordinary screen time.

A **group** = a family-chosen name + a hand-picked set of apps + one policy:

| Policy | Meaning | Allowance | Rides (wire) |
|---|---|---|---|
| **Free** | doesn't spend the day | optional "free up to N min/day" | `learning` (key 5) |
| **Counted** | its own allowance inside the day | daily and/or weekly minutes | `buckets` (key 10) |
| **On request** | closed until asked-for and granted | per-grant window | `apps` (key 4) blocked + holds |

An app belongs to **at most one group** (editor-enforced). Ungrouped apps are
ordinary screen time. Per-app limits need no feature: a one-app Counted group
*is* "Minecraft: 1h/day". Site-app identities (pinned Chromium windows) group
exactly like apps — same identity vocabulary.

This is Approach A: the UI unifies **now**; the wire keeps the existing clauses
underneath, which preserves the deliberate fail-safe asymmetry for free —
Free groups fail **closed to Screen** (an error must never make time free),
Counted groups fail **open** (an error must never confiscate), On request fails
**closed** (blocked stays blocked, the existing apps-clause direction).

## 2. Wire changes (all additive; spec/contract.md updated alongside)

### 2.1 `buckets` (key 10) — weekly allowance
`AppBucket` gains `weeklyMinutes?: 1..=10080`; `dailyMinutes` becomes optional;
**at least one required**. Both set ⇒ both caps apply; whichever exhausts first
closes the group. Week convention is **identical to the budget clause**: the
ledger's existing `week_key_of` (calendar-day walk-back to `weekStart`, default
Mon, enforcement tz). `GrantBuckets` gains `weekStart?: 'sun'|'mon'` mirroring
`GrantBudget`. Ledger grows `bucket_week_secs` beside `bucket_today_secs`,
zeroed on week roll.

**Versioning rule (compat):** the PWA emits `v:1` whenever every bucket has a
daily value (old wards enforce the daily half and ignore the unknown weekly
field). If any bucket is **weekly-only**, the PWA emits `v:2`; old wards fail
open (enforce nothing) until updated — honest per the buckets fail-open
doctrine, and surfaced in the editor's parity note.

### 2.2 STATUS — group progress for the guardian
STATUS gains `groups?: [{ id, daySecs, weekSecs }]` (spent seconds, day- and
week-keyed, capped list). Guardian UI can finally show "Play: 45m of 60m".
Free-group time keeps riding the existing `learningTodaySecs`.

### 2.3 Asks — `time.extend` learns group vocabulary
`LimitHit` gains `'bucket'`; `TimeExtendRequestParams`/`GrantParams` gain
`bucketId?` (echoed verbatim in the grant, like `limitHit`). Device-side, the
extension ledger's `Dimension` gains `Bucket(id)` — granted minutes top up that
group only, today-only, idempotent by reqId, same as ever. The PWA is a
deployed web app, so the guardian side understands `'bucket'` the moment we
ship; no stale-guardian window.

### 2.4 Gift (key 12) — give time to a group
`GiftBody` gains `groupId?`. Present ⇒ the minutes land in `Dimension::Bucket`
for that group; absent ⇒ exactly today's behaviour. Stays `v:1`: an old ward
ignores the unknown field and the gift lands as device time — errs generous
(never "Mum gave me time and nothing happened"), window closes with the ward
update, noted in the parity copy.

### 2.5 On request — new ask op + a presentation hint
- New op `app.open` with `AppOpenRequestParams { pkg, label?, minutesRequested?,
  reason? }` (same envelope, caps, reqId/nonce rules as `time.extend`).
- **The grant is not a new wire object**: the guardian's one-tap answer re-signs
  the `apps` clause with an `AppHold { pkg, Allowed, untilUnix }` — the exact
  machinery shipped for "allow Vanadium for an hour". A `Decision` echo
  (allow/deny) closes the loop in the ward's UI.
- `GrantApps` gains `askFirst?: string[]` — a pure **presentation hint**. On-
  request apps are listed in `blocked[]` (so every ward, old or new, enforces)
  **and** in `askFirst[]` (so updated wards render "Ask to open" instead of
  "Blocked" and offer the ask affordance). Old wards: blocked, no ask button —
  fail closed, never fail open.

### 2.6 Free groups — names stay guardian-side this round
Multiple Free groups compile to **one merged `learning` clause** (union of
apps; a single optional shared `capMinutes` stays an advanced knob). Group
names for Free groups live in the PWA's guardian-side model (precedent:
`deviceOverrides`, which never reaches the wire). The ward shows one merged
Free meter this round; per-free-group meters on the ward are explicitly
deferred. Counted and On-request groups carry their names on the wire already
(`AppBucket.label`; app labels).

## 3. Enforcement

### 3.1 Android — closing the parity gap (the headline build item)
Buckets go from stored-inert to enforced:
- **Meter:** in `WardenController.tickAndApply`, the existing
  `usage.foregroundPackage()` probe attributes the tick;
  `charter-schedule::credit_app_bucket` (day + new week counters) via
  `warden.rs`, same crate the Linux warden uses.
- **Enforce:** spent groups fold their packages into the existing
  `appSuspendSet` / `AppGateOps.reconcile` path — same suspend mechanism as the
  `apps` clause. Spending a group suspends *that group's apps*; the device
  never locks for a group.
- **Ward mirror:** the status screen gains per-group rows ("Play — 45m of 1h
  left today, 2h this week") and "Ask for more" per spent group; on-request
  apps listed with "Ask to open".
- `wardenSupport` flips `buckets` from `android: NEVER` to `versionCode ≥` the
  shipping ward, so the parity note self-lifts as wards update.

### 3.2 Linux
- Weekly: `spent_bucket_apps`/`bucket_remaining_secs` learn the week cap
  (min of the two remainders); `BucketView` gains week fields (serde defaults
  `-1` = unset, per the established readout convention — never 0).
- Extension ledger `Dimension::Bucket(id)` consumed in the tick's pool math.
- Gift routing honours `groupId`.
- Tray/console: per-group "Ask for more" (sends `time.extend` with
  `limitHit: 'bucket'`), on-request apps listed with "Ask to open"
  (sends `app.open`).

### 3.3 Ordering invariants (from prior rounds, preserved)
Stand-down still caps everything — group time, gifts, and extends sit under it.
Group extends/gifts are additive pools *for that group only* and never touch
the device budget. Bucket time still also charges the day (an hour of Play is
an hour of the day). The one-X-probe/one-attribution rule stays: a tick credits
exactly one bucket.

## 4. Guardian PWA

One **"Named times"** section on Limits (Time tab) replacing the separate
Learning and App-time-limits sections:
- Group cards: editable name; policy segment **Free | Counted | On request**;
  for Counted, two steppers — *per day* and *per week* (each optional, ≥ one,
  15-min steps); app chips from the merged reported inventory + the free-text
  fallback for unreported devices.
- **Create flow:** "+ Named time" opens a card with tappable *suggestion* chips
  — Play, Learning, Social, Creative — plain editable text, no apps pre-sorted,
  no behaviour attached to the name. Suggestions teach the concept; the family
  still does the deciding.
- One-group-per-app: picking an app already in another group moves it (with an
  inline "moved from Play" undo), never duplicates.
- Progress from STATUS: "Play: 45m of 60m today · 2h10 of 5h this week".
- Asks inbox: a `bucket` extend renders "More **Play** time?" with the group's
  remaining context; `app.open` renders "Open **Minecraft**?" with one-tap
  windows (30m / 1h / today), which sign an `AppHold`. GiveTime gains an
  optional group picker.
- Parity notes per device stay honest: weekly-only groups need the updated
  ward; Android needs the new APK for any group enforcement; group names on
  Free time are guardian-side.
- Save bar unchanged: one save signs every touched dimension
  (learning + buckets + apps), as today.

## 5. Testing & hardware gates

- Core: vector tests for every wire addition (`weeklyMinutes` v:1/v:2 rule,
  `groupId` gift, `bucket` limitHit, `app.open`, `askFirst`); ledger tests for
  week roll, dual-cap min, `Dimension::Bucket` idempotence; the existing
  buckets/learning/apps invariant tests must stay green untouched.
- Linux: headless gates (fmt/clippy/test/real-build) + the tray render probe.
- Android: unit + instrumented where CI-less (Kotlin/JNI are NOT in CI — verify
  locally); audit any new instrumented `@Before` grants against
  `WardenController` (tests-that-grant-what-prod-forgets).
- **Hardware:** the test phone is attached — I drive adb myself: install ward
  APK, author a Counted group + an On-request app from the PWA, verify
  suspend-on-spend, ask-to-open round trip, gift-to-group, week counter
  survives day roll. Linux verified on this laptop's charterd.

## 6. Deferred (named so they aren't forgotten)

- Per-free-group meters/names on the ward (wire: per-app group label on
  `learning`).
- Per-group weekday shapes (school-day vs weekend *within* a group).
- A unified `groups` wire clause retiring 5+10 (Approach B) — can land under
  this UI later without the UI changing.
- Per-group progress in the weekly picture/history charts.
- Asks proposing *new* groups (ward suggests "can Minecraft be Creative?").
