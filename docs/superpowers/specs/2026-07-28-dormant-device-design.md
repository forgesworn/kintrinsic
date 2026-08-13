# A device that is off until the guardian opens it

**Date:** 2026-07-28
**Status:** approved (decented, 2026-07-28)
**Prompted by:** the Blue tablet — a Wi-Fi-only Galaxy Tab A9 (SM-X110) provisioned
as its own ward the same day.

## The situation

The family has a spare tablet. It is nobody's daily driver. Both children use it
occasionally — a special occasion, a flat battery on their own phone — and a
visiting child might use it too. It is paired as its **own ward** ("Blue tablet"),
which is the right call: a guest is not in the family's charter and should not
have to be.

Being its own ward is also the hazard. A ward has its own allowance, so a child
who has spent their hour on their own phone could pick up the tablet and find a
second, unrelated pool. That is how a spare device becomes the sneaky device.

What decented wants: **permanently off, opened deliberately, closing itself again.**

## Why the obvious route fails

The daily-limit stepper in MyCharter has `min={15}`, so the smallest standing
allowance a guardian can express is fifteen minutes a day. Fifteen minutes a day
is not off — it is a small daily invitation to go and check.

Lowering that floor to zero does not work either. `budgetToGrant` maps the app's
`Budget.paused` to an *unconstrained* budget, because in the app's domain model
"paused" means "the cap is lifted" — the exact inverse of the wire's
`GrantBudget.paused`, which means quota 0. The inversion is deliberate and
documented in `apps/charter-app/src/wire/types.ts`. There is no domain state that
reaches wire budget-paused, and adding one would put a second, opposite meaning of
the same word into a file that exists to keep them apart.

## What already works

The capability exists and is reachable today, by a different door.

`scheduleToGrant` (`apps/charter-app/src/wire/clause.ts`) detects a schedule that
allows zero time anywhere — every day empty, no override windows — and encodes it
as an explicit wire `paused: true`, precisely so the device does not read an
all-empty `weekly` as "no schedule, always allowed". `evaluate_grant_schedule`
returns `Locked { seconds_to_open: None }`. The device is off, indefinitely, with
no standing pool to discover.

Opening it also works. A guardian's **Give time** gift routes to whichever
dimension is holding the ward out — schedule lock ⇒ schedule pool — and
`compute_remaining` adds the pool on top of zero. The extension cannot push past
end-of-day and the gift carries its own `expiresAt`, so the tablet returns to
dormant overnight without anybody remembering to close it. The ward's own **Ask
for more time** button routes by the same rule (`LockActivity.kt:596`).

So the enforcement path needs no new clause, no new field, and no new hardware
verification. The whole gap is that the tablet lies about itself and the guardian
cannot ask for this posture by name.

## The lie

On a dormant device the shade currently reads:

> **Outside allowed hours**
> Access resumes during your scheduled time.

There is no scheduled time and none is coming. `lock_info` cannot produce a
"back at…" line either, because `next_open_secs` is `None` — so its fallback,
*"You can come back when your next window opens."*, repeats the same false
promise. A child is told to wait for something that will never happen.

The guardian side is quieter but similar: `scheduleSummary` renders zero open days
as **"No time allowed yet"**, which reads as an unfinished setup rather than a
decision, and there is no control that says "this is how I want it".

## The design

Name the posture at both ends. Change no enforcement.

### 1. The wire and the enforcer: unchanged

Dormant *is* wire `GrantSchedule.paused == true`. No new clause kind, no new
field, no new lock reason. This is the load-bearing constraint of the whole
design — see the trap below.

### 2. Core — honest copy (`charter-spine`)

`lock_message(reason)` gains a sibling:

```rust
pub fn lock_message_for(reason: LockReason, dormant: bool) -> (&'static str, &'static str)
```

which returns the dormant copy when `reason == Schedule && dormant`, and
delegates to `lock_message` otherwise. `lock_message` keeps its current signature
and behaviour so no caller is forced to change before it has a schedule in hand.

Dormant copy:

> **This device is off**
> Ask your guardian to open it for a while.

`week_lines` is suppressed for a dormant schedule: seven lines of "no screen
time" is noise that says nothing the title has not.

### 3. The ward's shade (Android)

Two changes in `android/jni/src/warden.rs::lock_info`:

- the `"schedule"` arm re-derives its own copy in Rust rather than using the
  shared strings (a pre-existing drift — the `"standdown"` arm was already fixed
  this way after a stood-down ward saw a shade headlined "Unlocked"). Route it
  through `lock_message_for` and the drift closes as a side effect.
- pass dormancy, read from the effective schedule.

In `LockActivity.kt`, the ask button reads **"Ask to use this"** when the device
is dormant and keeps **"Ask for more time"** otherwise — a child who has had no
time cannot ask for *more*.

### 4. The guardian (MyCharter)

Dormancy is **derived, never stored**: a schedule with zero windows anywhere and
not app-paused is dormant. A stored flag would be a second source of truth that
could disagree with the clause actually on the wire, and would not survive a
guardian editing the schedule on another device.

- `scheduleSummary` renders that state as **"Off — you open it when needed"**.
- A control in the schedule editor states the posture: **"Off unless I open it"**.
  Turning it on clears every window; turning it off restores a default week so
  the guardian is not left staring at a blank editor.
- The device card gains a **Give time** affordance for the dormant case, which is
  the only way in and should not be buried under a schedule the guardian has
  deliberately emptied.

### 5. What we are deliberately not building

- Cross-ward accounting. Time on the Blue tablet does not draw down a child's own
  budget. Charter pools usage per child across *their* devices, not across
  subjects, and making it do otherwise means the tablet must know which child is
  holding it. decented's call: he is the gate, and granting time is a knowing act.
- Friction on asking (hold-to-ask, one ask a day). No evidence it is needed.
- A bespoke device type. Live with the simple version first.

## The trap

The obvious implementation is a new `LockReason::Dormant`. **Do not.**

Gift and ask routing both switch on the lock reason, in three places:

- `linux/crates/charterd/src/runtime.rs:1247` — `Schedule ⇒ Dimension::Schedule`, else Budget
- `android/jni/src/warden.rs:1428` — same rule, expressed via `ScheduleStatus::Locked`
- `android/app/src/main/kotlin/org/forgesworn/charter/ui/LockActivity.kt:596` — a
  string comparison, `reason == "schedule"`

A new reason that any one of those does not know about routes the grant to the
**budget** pool, which cannot open a schedule-locked device. The guardian taps
"Give 30 minutes", MyCharter reports it sent, and the tablet stays locked — a
silent failure with no error anywhere. Keeping the reason as `Schedule` and
varying only the *message* leaves all three untouched.

## Testing

- `lock_message_for`: dormant schedule ⇒ the off copy; non-dormant schedule,
  budget, stand-down, malformed ⇒ unchanged from `lock_message`.
- `lock_info`: a dormant schedule emits no week lines and no "back at" detail.
- A dormant schedule still routes a gift to the schedule pool and the device
  opens for exactly the gifted minutes (guards the trap above).
- `scheduleToGrant`: zero windows ⇒ `paused: true` (existing behaviour, pinned).
- `scheduleSummary`: zero windows ⇒ "Off — you open it when needed"; app-paused
  ⇒ unchanged.
- The posture control clears windows on, restores a default week off.

## Hardware gate

Only decented can run this, on the Blue tablet:

1. Set **Off unless I open it**; confirm the shade reads "This device is off" and
   offers "Ask to use this" — not "Outside allowed hours".
2. **Give time** 10 minutes from MyCharter; confirm the tablet unlocks and the
   countdown reads ~10 minutes.
3. Let it run out; confirm it re-locks to the same dormant copy.
4. Tap **Ask to use this**; confirm the request reaches MyCharter and that
   approving it opens the tablet.
