# Times — Free time, Learning time and other named times

Design spec, 2026-10-30. Decided in a design session with the founder; this
document is the source the build works from. The wire shapes it introduces are
normative in `spec/contract.md` (§Times); where the two disagree, the contract
wins.

## The problem it solves

A guardian's son has a laptop and a phone. His "free time" is 16:00–19:00 on
both. Every weekday morning the guardian taps **More time** so he can do Khan
Academy on the laptop, and trusts him not to open Minecraft or entertainment
YouTube — because the morning top-up unlocks *everything*, on *both* devices.

Three deliberate properties of the contract line up to produce this:

1. Clauses are per-child; the gift goes to every device.
2. A gift is minutes against the wall and carries no "what".
3. Time (the schedule) and content (apps, web) are orthogonal and both
   standing. Nothing binds a time box to an allowed set.

The model is coherent. The *lever* is child-wide and time-only, and that is the
wrong shape for "two hours of maths on the laptop".

## The model

A **time** is a named kind of screen time with its own posture. A device is in
exactly one time at any instant, or it is locked.

1. **Free time is the background.** Every child has it; it cannot be deleted
   or renamed in v1; it is "everything except what's banned". Its bans are the
   existing standing clauses (`apps`, `content`, `apprules`, `buckets`,
   `learning`). Outside any time the device is locked, exactly as today.
2. **Named times carve out of it.** *Learning time* ships pre-made as a
   template; any named time can be renamed; more can be added. Each has a
   posture: **only these** (an allow-list of apps and sites) or **everything
   except** (Free time plus departures either way).
3. **Definition per child, windows per device.** "Learning time = Khan
   Academy + YouTube" is defined once (the `times` clause, per child). *When*
   it runs is a tagged window in each device's own `schedule` clause (the
   schedule is already split per device guardian-side).
4. **Inherit, don't copy.** An *everything except* time inherits Free time's
   bans live and can override either way: ban more, or un-ban. A ban added to
   Free time next month applies during Learning time without the guardian
   re-entering it. An *only these* time lists what runs; Free time's app lists
   are not consulted (listing something is the un-ban). Overarching rules —
   `apprules` (per-app blocks and hours), `buckets` caps, `hidden` apps, and
   `alwaysavailable` exemptions — apply in every time and cannot be overridden
   by a named time.
5. **Overlap: a named window beats a Free window** where they overlap (a
   homework hour inside free time carves it). Two *named* windows overlapping
   on one device is refused by the editor; a device that nevertheless receives
   one resolves deterministically (earlier `start` wins; ties go to array
   order).
6. **Extensions carry a type and a device.** "More time" becomes "More
   [Free | Learning | …] time", wrapped only to the chosen device(s). The
   ward's own ask picks a type the same way. Minutes land in a per-time pool;
   out of any window, the device runs the time whose pool it is burning.
7. **Budget.** A named time does not drain the daily/weekly budget unless its
   definition says so (`countsAgainstBudget: true`), and budget exhaustion
   does not lock a named time that does not count.
8. **The ward side is half of it.** The lock card and status show which time
   the ward is in, what is allowed, and what is next: "Learning time until
   12:00 — Khan Academy, YouTube"; "Locked until Learning time at 09:00".
9. **New-family default:** Free time with a schedule, nothing else. "Add a
   time → Learning time / Custom" is opt-in.

### The fold: the old "Named times" screen

The guardian app has an existing *Named times* screen: groups of apps with a
policy of Free (time-free, the `learning` clause), Counted (`buckets`) or On
request (`apps.blocked` + `askFirst`). Those are app-level allowances inside
Free time, not windows. They keep their wire shapes and their editor, and move
under **Free time → Apps** as *App allowances* (Time-free / Counted / Ask
first). The words "named time(s)" leave that screen's copy entirely; a *time*
now always means a window with a posture.

## Wire (normative text lives in `spec/contract.md`)

All changes are additive at `v: 1` except the new `times` clause.

### `times` clause (tag `"times"`, store key `17`)

```ts
type TimePosture = 'everythingExcept' | 'onlyThese';

interface NamedTime {
  /** [a-z0-9-]{1,32}; unique within the clause; never "free" (reserved). */
  id: string;
  /** Display name, 1..60 chars. */
  label: string;
  posture: TimePosture;
  // everythingExcept — departures from Free time:
  blockApps?: string[];   // additionally blocked (AppRule.pkg vocabulary)
  unblockApps?: string[]; // removed from Free time's `apps.blocked`
  denySites?: string[];   // additionally denied registrable domains
  undenySites?: string[]; // removed from Free time's `parentDeny` / category blocks
  // onlyThese — the whole statement:
  apps?: string[];        // the only apps that run (plus implied runtime, below)
  sites?: LearningApp[];  // kind 'site' only: id, label, url, verified domains closure
                          // — the learning clause's own shape, so Linux can pin
                          // and launch each one; Android allowlists ⋃ domains
  /** Default false: time here does not drain the daily/weekly budget. */
  countsAgainstBudget?: boolean;
}

interface GrantTimes { v: 1; times: NamedTime[]; issuedAt: number; }
```

Replace-the-set, per child, LWW by `issuedAt` under the usual rollback
protection. Up to 16 times. Fields for the other posture are ignored. An
*only these* time with neither `apps` nor `sites` is valid on the wire and runs
nothing (the editor refuses to save one).

### `ScheduleWindow.time?: string`

A window tagged with a named-time id runs that time; untagged = Free time.
Evaluation at `now` (device, every tick):

1. Take today's windows (override for the date, else weekly).
2. If `now` is inside a tagged window → that time (ties: earlier `start`,
   then array order). Else if inside an untagged window → Free. Else → locked
   by schedule.
3. A tag whose id is not in the current `times` clause, or no `times` clause
   at all → that span is **locked** ("Nothing is set up for this time yet").
   A named window is a restriction relative to Free; opening it as Free would
   grant exactly what the guardian meant to withhold.

A pre-times ward ignores `time` and runs the window as Free — a loosening,
version-gated in the guardian app (`wardenSupport.times`: Android ≥ 49, Linux
≥ 800) exactly like every other additive field.

### `GiftBody.time?: string`, `TimeExtendRequestParams.time?`, `TimeExtendGrantParams.time?`

The id of the time the minutes buy. Absent = Free (today's behaviour,
byte-identical). The grant echoes `time` verbatim from the request (the
params-echo check is exact). To change the type when answering an ask the
guardian app sends a `minutesGranted: 0` grant (closing the ask) plus a gift
with the wanted `time`, both wrapped to the asking device only. On a new ward
a gift or grant naming an unknown id enacts **no extra time** (a loosening
with an unenforceable posture fails safe). A pre-times ward ignores the field
and lands the minutes as Free, the same documented gap as `groupId`.

### Per-time extension pools

The `ExtensionLedger` gains pools keyed by time id, alongside the existing
`schedule`/`budget` pools (which remain Free's). Inside a window that can run,
the window decides and no pool burns. When it cannot — outside every window,
or the window's time is Free (or counts against budget) and the budget is
spent — the device falls through to the pools: Free's pool for the locking
dimension first (today's routing), then the named pool most recently topped
up that still has minutes, run *as that named time*. So 30 minutes of
Learning time given at 17:30 to a budget-exhausted child unlocks the laptop as
Learning time. `windowLeftSecs` is the seconds to the end of the contiguous
same-time span. Pools are today-only and idempotent by id, as before.

### STATUS

`activeTime?: string` — the id of the named time in force; absent when Free
or locked. Pre-times payloads stay byte-identical.

### Composition (pure, shared crate, both wardens)

Given Free time's resolved standing clauses (`apps` after
`effective_at(now)`, `content`) and the active `NamedTime`:

- **everythingExcept**: `apps.blocked := (blocked ∪ blockApps) \ unblockApps`
  under Free's posture; `content.parentDeny := (parentDeny ∪ denySites) \
  undenySites`, `content.parentAllow := parentAllow ∪ undenySites`. Everything
  else carries unchanged.
- **onlyThese**: `apps.posture := allowlist, allowed := apps`. If `sites` is
  non-empty: `content.posture := allowlist, curators := [], parentAllow :=
  ⋃ sites[].domains`, SafeSearch and YouTube-restrict carried from Free, and
  the device's **sanctioned web runtime is implied allowed** (Linux:
  resolver-pinned site windows rendered per site, as the `learning` clause
  already does, with unsanctioned Chromium swept exactly as when any site app
  is in force; Android: the device's default browser package). If `sites` is
  empty, Free's web rules carry and no browser runs unless listed in `apps`.
- **Linux, inventory unknown:** an *only these* time is locked (the allowlist
  is inventory-scoped and "empty inventory blocks nothing" would run it as
  Free); `lockReason: 'schedule'`. Same fail direction as the learning arm.
- In all cases: `apprules`, `buckets`, `hidden`, `alwaysavailable` apply
  unchanged on top; sanctioned learning windows survive (existing rule).
- `countsAgainstBudget: false` → usage in this time charges neither the
  screen budget nor any bucket; budget exhaustion does not lock it.

## Ward-side behaviour

- **Lock card / lock screen:** "Locked until <Time> at HH:MM" for the next
  window today (else tomorrow's first; else "Locked today"). Ask-for-more-time
  offers a type when the child has any named time; default = the time whose
  window most recently ended today, else the next one up today, else Free.
- **Unlocked in a named time:** status/tray (Linux) and the ward app's home
  (Android) show "<Time> until HH:MM" and, for *only these*, the allowed list.
  10-/1-minute warnings name the time and say what follows ("then Free time"
  / "then locked").
- **Transitions** between times are immediate on the tick, as lock/unlock
  already are; a running app that is not allowed in the new time is swept by
  the existing per-app machinery.
- **Linux web caveat (unchanged):** Firefox `policies.json` is machine-wide,
  so an *everything except* site override re-renders it at each transition and
  affects every account. *Only these* uses per-user site windows and does not
  touch Firefox policy. YouTube restricted mode is still not enforced on Linux
  (separate resolver build).

## Guardian app

- **Child settings → Times.** Free time card (schedule, per-device split as
  today; its rules are the existing Rules sections, relabelled as Free time's)
  and one card per named time; **Add a time** (Learning time template /
  Custom). Learning time template: posture *only these*, no apps, sites
  pre-seeded from the learning catalogue picks, `countsAgainstBudget: false`,
  no windows.
- **Time editor:** name, posture, apps (from each device's inventory and the
  launch-signature catalogue), sites (domain list + catalogue), counts-against-
  budget toggle, and windows per device. Windows are written as tagged
  `ScheduleWindow`s into each device's schedule; the schedule editor draws
  tagged windows distinctly, lets a named window overlap Free (carving), and
  refuses two named windows overlapping on one device.
- **Now card:** shows the active time ("Learning time · 1h 20m left").
  **More time** → type picker (default per the ward rule above) + device
  picker (default: devices STATUS currently reports locked; all when unknown);
  the gift is wrapped only to the chosen devices.
- **Asks:** show the requested type; approve as asked, or change type (0-grant
  + gift, per the wire section).
- **Gating:** `wardenSupport.times = { android: 49, linux: 800 }`. On an
  ungated device the Times screen explains the device needs updating and does
  not offer tagged windows or typed gifts for it. **A tagged window never
  reaches an ungated device:** the Time editor lists only gated devices, and
  writing a tagged window into a device's schedule while the schedule control
  is still shared first splits it (seeding every device with the shared value,
  `setControlSplit`), so the ungated siblings keep an untagged schedule.
- **Deleting a named time** removes its tagged windows from every device's
  schedule in the same signed save (one batch), so no device is left locked
  on an id that no longer exists. Renaming keeps the id; only `label` changes.
- **Fold** as described above; no other change to the existing Rules editors.

## Constants (§5.6 parity, both stacks)

`MAX_NAMED_TIMES = 16`, `MAX_TIME_LABEL_LEN = 60`; store key `17`; id pattern
`^[a-z0-9-]{1,32}$`, `free` reserved.

## Fail-safe table

| Situation | Device behaviour |
|---|---|
| `times` clause unparseable / `v` too high | treated as absent: every tagged window locks, untagged unchanged |
| tagged window, id unknown | locked for that span |
| two named windows overlap | earlier `start`, then array order |
| *only these* with empty `apps` and `sites` | unlocked, nothing runs (editor refuses) |
| gift / grant `time` unknown | no extra time |
| pre-times ward | tag/field ignored → Free (version-gated guardian-side) |
| `countsAgainstBudget` absent | false |

## Out of scope (recorded, not forgotten)

- Renaming or deleting Free time.
- An un-overridable "always" layer above Free time (safety categories no
  named time can un-deny). Not yet; Free time's bans are the baseline.
- YouTube restricted mode on Linux (resolver applier).
- Lock-except-app on Linux (whole-slice freeze stays).
- The `src/` consumer SDK's older schedule shape.
- Shared device that takes on whoever is using it (docs/guardian-ux-design.md).

## Release

Ships as ward 0.7.0 (49), charterd 0.8.0 (800), guardian 0.2.0 (23), PWA on
push to `main`. Hardware gates for the founder are listed in the PR.
