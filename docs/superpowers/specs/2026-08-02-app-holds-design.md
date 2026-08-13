# Temporary app holds — "allow it for an hour"

**Date:** 2026-08-02
**Status:** approved (decented, 2026-08-02)

## The problem, exactly as it happened

decented needed to let Robin onto Vanadium. The only lever MyCharter offers is
the standing per-app toggle in **Limits → Apps & web → Apps**, so he flipped
Vanadium off the blocklist. Vanadium is now open — and will stay open until
decented *remembers* to go back and flip it on again.

That is a rule change used to solve a one-off. It is the same failure the `gift`
clause was built for on the time dimension ("finish the level" used to mean
editing the schedule), and the same one the install window was built for on the
install dimension. The apps dimension is the one that never got its temporary
form.

The standing toggle is not wrong; it is simply the wrong *duration*. What is
missing is a way to say **"for an hour"** and have the phone put it back.

## What we are building

Beside every app's toggle in that list, a second control — a **hold**. Pressing
it opens a small sheet offering fixed durations. Picking one signs and sends a
clause immediately; the app's row then shows the state and the time remaining,
and the phone reverts by itself when the clock runs out.

A hold works in **both directions**, because the list works in both directions:

- a **blocked** app gets **"Allow for a while"**
- an **allowed** app gets **"Block for a while"**

decented's answers, 2026-08-02:

1. **Live immediately** — one press signs and sends. Not a draft waiting on Save.
2. **Fixed presets** — 15m / 30m / 1h / 2h / Until bedtime. No stepper.
3. **The ward is told** — a notice on the phone when a hold starts and when it ends.

## Why a hold lives in the `apps` clause, and not in MyCharter

The obvious cheap build is a timer in the PWA that re-signs the reverse clause
when it fires. It is wrong, and it is worth saying why once so nobody proposes
it again: MyCharter is a web app on a guardian's phone. The OS freezes its
timers the moment it is backgrounded (this exact trap already cost us a
day — see `mycharter-staleness-traps`). A hold whose expiry depends on the
guardian's phone being awake is a hold that silently never ends. The loosening
would outlive the guardian's intent, which is the worst direction to fail in.

So the expiry is **carried on the wire as an absolute instant** and the **ward's
own device** enforces it — the same shape as `tethering.until` and
`maintenance.untilUnix`, both of which are absolute for the same reason (a
duration restarts every time the stored clause is re-read, and the window never
shuts).

## Wire

`GrantApps` (wire tag `apps`, store key 4) gains **one optional field**. The
clause stays `v: 1`: an older ward that does not know `holds` simply ignores it
and keeps enforcing the standing lists. MyCharter guards that case separately
(see *Version gate*).

```ts
/** A time-boxed departure from the standing per-app lists, for one app. */
interface AppHold {
  /** On-device identity — same vocabulary as AppRule.pkg. */
  pkg: string;
  /** What this app is, until `untilUnix`. */
  state: "allowed" | "blocked";
  /** Absolute unix seconds. The hold ends AT this instant. Never a duration. */
  untilUnix: number;
}

interface GrantApps {
  v: 1;
  posture: "blocklist" | "allowlist";
  blocked?: string[];
  allowed?: string[];
  paused?: boolean;
  /** Time-boxed overrides. Absent/empty = the standing lists alone. */
  holds?: AppHold[];
  issuedAt: number;
}
```

### Resolution — one pure function, shared

The device never enforces `GrantApps` as authored. It enforces
`effective_at(now)`, a pure function of the clause and the clock:

1. **Drop every dead hold** — `untilUnix <= now`.
2. **Drop every implausible hold** — `untilUnix > issuedAt + 24h`. A clause
   claiming a three-year allowance is either a bug or an attack, and the
   fail-closed reading of an over-long *loosening* is to refuse it. The cap
   mirrors `MAX_MAINTENANCE_SECS` in intent.
3. **Drop past the 64th hold.** Bounds the clause; 64 held apps at once is not a
   family, it is a bug.
4. **Apply what survives**, last-writer-wins per `pkg`:
   - *blocklist posture:* `allowed` removes `pkg` from `blocked`;
     `blocked` adds it (deduped).
   - *allowlist posture:* `allowed` adds `pkg` to `allowed` (deduped);
     `blocked` removes it.
5. Return the resolved clause with `holds` cleared.

`paused` still wins over everything — the section's "off" lifts holds too, which
is right: nothing in this clause is being enforced at all.

Because resolution is level-triggered from an absolute instant, a reboot inside
a hold does not extend it by a second, and a device that was switched off
through the whole hold comes back to its standing rule with nothing to undo.

**Clock skew.** A device whose clock is behind sees the hold as still live; one
ahead sees it as already dead. There is no defence against this that does not
introduce a worse dependency (a trusted time source the phone may not be able to
reach). It is the same exposure `tethering.until`, `maintenance` and stand-down
already carry, and the blast radius is one app for at most a day.

## Android (the enforcing side)

`Warden::app_policy()` becomes `app_policy(now_unix)` and returns the
**resolved** clause. Every existing consumer keeps working unchanged: Kotlin's
`appSuspendSet` still reads `posture` / `blocked` / `allowed` and unions with the
schedule-driven `app_rule_suspensions`, exactly as now. A hold is invisible to
the enforcement path — it has already become a list.

That is the whole enforcement change. The union logic, the DPM reconcile, the
listening exemption and the lock surface are untouched.

### Telling the ward

A new `Warden::app_holds(now_unix)` returns the live holds as JSON. Each tick,
`WardenController` diffs it against a persisted last-seen map and posts a notice
on change:

| Transition | Notice |
| --- | --- |
| hold appears, `state: allowed` | "Vanadium is open until 8:15 pm" |
| hold appears, `state: blocked` | "Vanadium is paused until 8:15 pm" |
| hold ends, app now blocked | "Vanadium is closed again" |
| hold ends, app now open | "Vanadium is open again" |

The end-state wording is read from the resolved policy, not assumed from the
hold's direction, so a hold that expires into a standing rule the guardian
changed mid-hold still tells the truth.

The label comes from `PackageManager`; the time from
`DateFormat.getTimeFormat(context)` so it reads in the ward's own locale and
clock format. New channel `charter-apps` ("App changes"), `IMPORTANCE_DEFAULT` —
this is information, not an alarm.

The last-seen map is written **only when it changes**, never per tick — the
background-power pass (`background-power-shipped`) named per-tick flash writes as
one of the five drains, and this must not reintroduce it.

## MyCharter (the authoring side)

### The row

Each app row in `AppsEditor` becomes:

```
Vanadium                              [⏱]  [ ON ]
  org.chromium.vanadium
```

and, while a hold is live:

```
Vanadium              Allowed · 59 min left
  org.chromium.vanadium               [⏱]  [ ON ]
```

The `⏱` button's accessible label follows the app's *effective* state — "Allow
Vanadium for a while" when it is blocked, "Block Vanadium for a while" when it
is not — so a screen reader never announces the opposite of what the press does.

### The sheet

```
┌──────────────────────────┐
│ Allow Vanadium for…      │
│  [15m] [30m] [1h] [2h]   │
│  [ Until bedtime ]       │
│         [Cancel]         │
└──────────────────────────┘
```

**Until bedtime** resolves to the end of the last allowed window in today's
schedule. The preset is **hidden** when that cannot be computed — no schedule, or
the last window has already passed. A button that silently means "midnight" when
it says "bedtime" is worse than no button.

The absolute instant is computed from the *guardian's* clock. For a family in one
house that is correct; across time zones it is off by the offset. The install
window already carries this property and it has not bitten; noting it rather
than building a tz picker nobody asked for.

When a hold is already live the sheet instead reads *"Ends in 59 minutes"*, the
presets **replace** the remaining time (they do not add to it — "1h" always
means one hour from now, which is what the words say), and an **End now** action
clears it.

### Live immediately, without fighting the Save bar

The Limits screen signs one clause per save, carrying every changed dimension.
A hold that published on its own would race a pending draft and could revert it.

So the press does not bypass the machinery — it **drives** it:

1. The hold is applied through the *same* `onChange` the toggle uses. This
   matters: that path is what routes an edit into `deviceOverrides` when the Apps
   control is split per device, so holds inherit per-device splitting for free
   and no split-aware code is written twice.
2. A pending-save flag is set in the same event. An effect observes it after the
   draft state has settled and calls the existing `save()`.

The guardian presses once. The existing save path signs once.

Consequence, stated plainly in the sheet when it applies: **other unsaved changes
on the screen are saved too.** Copy — *"Your other unsaved changes on this screen
will be saved as well."* Shown only when something else is actually dirty.

### Two rules that keep the list honest

- **Toggling an app clears its hold.** Changing the standing rule for an app is a
  decision that supersedes the temporary one; leaving a hold behind would mean
  the toggle appeared not to work.
- **Expired holds are never rendered.** The row is level-triggered from
  `untilUnix` versus now, exactly like `installWindow()` — never a latched flag.
  They are pruned from the policy on the next publish.

### The countdown

A 5-second tick, running **only** while the Apps section is open, the document is
visible, and at least one hold is live. `holdLabel` rounds minutes **up** above
60 seconds ("59 minutes left") and reads "under a minute left" below it — never
"0 minutes", which reads as expired while the phone still allows it.

### Version gate

A new `wardenSupport` feature `appHold`, `{ android: 36 }`. A phone reporting an
older Charter is named in the sheet ("Rob's phone needs the latest Charter before
it can hold an app") rather than silently swallowing the field. Silence still is
not incapacity — an unreported device is assumed capable, per the existing rule.

## Known gap: Linux

`charterd` does not consume the `apps` clause at all today — only `appRules`
(store key 6). The standing toggles in this list already do nothing on a paired
laptop, and holds will inherit that. **This spec does not close that gap**; it is
pre-existing, it is a separate piece of work (Linux app suspension), and
widening scope to it would delay the thing decented actually asked for. It goes on
the roadmap.

`appHold` therefore declares no Linux threshold, because no charterd version
honours it and pretending otherwise in either direction would be a lie.

## Out of scope

- The per-app **policy cards** (app-scope `Policy.blocked`, the `appRules`
  clause) are untouched. decented asked for the button on "every app in the list",
  which is this list.
- No guardian-side notification when a hold ends. The remaining time is on the
  row; adding a push to the carrier app is a separate ask.

## Testing

**Rust (`charter-proto`)** — `effective_at`: allow-hold on a blocklist, block-hold
on a blocklist, both on an allowlist, an expired hold, one past the 24h cap, the
65th hold, duplicate `pkg`, and `paused` beating every hold.

**Rust (`warden.rs`)** — an `apps` clause with a hold drives `app_policy` at `t`,
and drives it back at `t + 1`; `app_holds` reports live holds only.

**Golden vectors** — `apps_clause_vectors.json` gains a hold vector, asserted
from both sides (TS producer, Rust consumer) as the existing three are.

**Kotlin** — `holdNotices(prev, next, blockedAfter)` as a pure function: appear,
disappear, change, unchanged (writes nothing), and process restart.

**TypeScript** — `pruneHolds`, `effectiveAppState`, `holdLabel`, `untilBedtime`,
`appsToGrant` carrying and pruning holds, and the toggle-clears-hold rule.

## Delivery

- Ward APK **0.6.5 / versionCode 36** — required; the feature does nothing on 35.
- MyCharter deploys on push to `main`. Its version is bot-cut; not hand-written.
- Two clocks: **code-ready** is a focused session. **Hardware-verified** needs one
  round from decented on Rob's phone — install 0.6.5, hold Vanadium for 15 minutes,
  confirm it opens, confirm the notice, confirm it closes on its own.
