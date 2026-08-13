# Always available apps

**Date:** 2026-08-03
**Status:** approved (decented, 2026-08-03 — "android only. this must not be
limited to listening apps, they may have an app this is 24/7 require. maybe a
messaging app while at a sleep over or something else.")

## The situation

A ward's phone is hers between 07:00 and 19:00. Outside that, the schedule
closes and the device locks. That is the arrangement the family wants, and it
works — except that a small number of apps are needed at any hour, and today
there is no way to say so.

The case that surfaced it: she wakes at 2am and puts on an audiobook to get back
to sleep, setting the app's own sleep timer. But the need is not about audio.
A messaging app at a sleepover is the same shape, and so is the next one nobody
has thought of yet. This clause is about **hours**, not about a category of app.

Today, none of it is possible, and three things that look like they'd help do
not:

- **The schedule lock takes everything.** `appSuspendSet` resolves `locked ->
  launchable.toSet()` (`android/app/.../enforce/Enforcement.kt:61`). Every
  launchable package is suspended, and the shade is over the screen.
- **`listening` is the wrong tool.** It exempts a named app only while
  `AudioManager.isMusicActive()` — audio *already playing*. It exists so a story
  mid-sentence can finish, and deliberately will not let her *start* one. At 3am
  nothing is playing.
- **Named-times "free" is about counting, not windows.** A free app still gets
  suspended when the window shuts.

## The decision

A new clause naming apps that are **open at any hour**, exempt from the schedule
and budget locks but from nothing else. Each entry may be standing, or carry an
expiry so a one-night grant expires on its own.

## The clause

Kind tag `"alwaysavailable"`, **store key 15** (next free; 14 is `listening`).

The tag is all-lowercase because that is what the wire already does —
`ClauseKind` carries `#[serde(rename_all = "lowercase")]`, which is why
`AppRules` is `"apprules"` and `StandDown` is `"standdown"` on both sides. The
Rust variant is `ClauseKind::AlwaysAvailable`; only the tag is flattened.

```ts
interface AlwaysAvailableApp {
  pkg: string;
  /**
   * Unix seconds; the entry ends AT this instant. Absent ⇒ standing, no end.
   * ABSOLUTE, never a duration — mirrors `AppHold`: a duration restarts every
   * time the stored clause is re-read and the grant would never end.
   */
  untilUnix?: number;
}

interface GrantAlwaysAvailable {
  v: 1;
  issuedAt: number;
  /** Apps openable at any hour. Empty ⇒ nothing is exempt. */
  apps: AlwaysAvailableApp[];
}
```

**Fails CLOSED.** This clause loosens enforcement, so absent, unparseable,
invalid or unrecognised resolves to an empty list — exactly today's behaviour.
Same discipline as `listening`; deliberately unlike `breakGlass`, which fails
open because there the harm is a ward stranded with no way out.

An expired entry is inert, not an error: the device drops it at read time and
the rest of the list stands.

## What the device does

A package escapes suspension when **all** hold:

1. the clause is valid and names the package
2. the entry has no `untilUnix`, or `now < untilUnix`
3. the lock reason is `Schedule` or `Budget` — **never** `StandDown`, **never**
   `Malformed`
4. the guardian has not blocked the package outright in the standing app policy

There is no audio condition, and that is the whole distinction from `listening`.

(3) is the boundary that keeps the clause honest. "Finish now" is the guardian
saying stop, right now; if an app could sit outside it the button stops meaning
anything — and the lifeline still reaches her either way. `Malformed` means
Charter cannot read its own rules, and a rule set we cannot trust cannot be
trusted to name a safe app.

`untilUnix` is resolved against the device's own clock at every tick, in the
shared `effective_at` alongside the existing app holds, so a reboot inside a
grant does not extend it by a second.

### Fixing the seam while we are in it

`appSuspendSet` documents condition (4) already — "an app the guardian blocked
outright stays blocked whether it is making a noise or not"
(`Enforcement.kt:71-73`) — but does not implement it. The final
`union - listeningExempt` subtracts from the *whole* union while locked,
including the standing blocklist, so a guardian-blocked app named in `listening`
is unsuspended at the lock. The guarding test passes `locked = false`
(`AppSuspendSetTest.kt:66`) and never exercises the case.

Narrow — it needs a guardian to both name and block the same app — but it is the
exact seam this clause extends. Fix it, and add the missing `locked = true`
case, in the same pass. Both exemption sets then subtract only from the
lock-driven posture, never from a standing block.

## How she opens it

The mechanism is already proven on hardware. The shade is LockTask-pinned, and
the lifeline puts the dialer on `setLockTaskPackages` so the in-call UI can
surface over the pin (`WardenController.kt:94-110`). Always-available apps ride
the same path:

- their packages join the LockTask allowlist
- the shade grows an **Open** row: app icons and names, nothing else. With no
  live entries the row is absent entirely — the shade must look exactly as it
  does today for a family that never sets this clause
- tapping launches the real app, full screen — her library, her sleep timer, her
  conversation
- leaving it returns her to the shade

`setLockTaskPackages` is called **once at init** today. The clause changes, and
entries expire, so it must become level-triggered like every other Charter
posture — re-derived each tick, including across a reboot.

Nothing else moves. Every other app stays suspended, the shade returns the
moment she leaves, and she cannot start anything that is not on the list. This
is a named door, not an unlock.

## Counting, and what gets seen

Time in an always-available app while the device is locked **and the screen is
interactive** is recorded as **out-of-hours use** — the name is deliberately not
about listening, because the clause is not.

The screen condition is load-bearing, not a detail. A sleep timer stops the
audio but leaves the app in the foreground, so counting regardless of screen
state would report eight hours for a thirty-minute session and make the line
useless in exactly the case it exists for. Counting interactive time only also
keeps this consistent with how Charter counts everything else: screen-off
listening has always cost a ward nothing.

"Out-of-hours" means *while locked*, whichever of the two exempting reasons is
in force. Use after the daily limit is spent at 5pm counts the same as use at
2am; there is one counter, not two. The weekly line is phrased by when it
happened rather than by which lock was standing, because the lock reason is
Charter's business and the pattern is the family's.

It does **not** accrue to her daily budget. Screen-off listening already costs
her nothing, and charging her for a bad night's sleep would be the counting and
the enforcing disagreeing.

The weekly picture gains one quiet line — e.g. "3 nights this week, ~40 min" —
and **the same line renders on her device** in the ward mirror. She knows it is
counted. Forty minutes once is a bad night; two hours every night is something a
guardian would want to see, and something she should be able to see about
herself. Transparency is the invariant, so there is no guardian-only variant of
this.

## The guardian's side

A section under **Limits → Apps & web**, above `Listening`:

```
Always available                    ›
Voice, AntennaPod, Signal (until Sun)
"Open at any hour, and never counted
 against her time."
```

The editor lists chosen apps with their expiry state — `always`, or `until
Sun 9am` — and an add-an-app picker drawn from the same reported inventory the
Apps section uses. No package names are hardcoded anywhere; they come from the
device's own inventory.

Setting an expiry uses the same absolute-instant control as app holds, so the
two read consistently to a guardian who has used one.

## What this honestly does not close

An always-available app is a **trusted** app, and the trust is real. Charter
cannot suspend a WebView inside an app it is deliberately allowing — a podcast
client renders show notes, a messaging app renders link previews. Two bounds
still hold: the device-wide DNS filter applies at 2am exactly as at 2pm, and a
link that tries to leave for a browser hits a suspended app and fails.

So it is bounded, not sealed. The guidance to a guardian should say so plainly:
choose apps you would be content with unsupervised at 2am. A local-file
audiobook player is the easy case; a messaging app is a real decision, which is
precisely why it can carry an expiry.

## Scope

**Android only.** Charter for Linux has the same schedule lock and the same
gap. Parity is a known standing debt and is explicitly a follow-on, not
something this pass covers or pretends to.

## Testing

- Pure decision fn: named + standing; named + unexpired; named + expired; not
  named; each lock reason (`Schedule`/`Budget` exempt, `StandDown`/`Malformed`
  not); malformed clause ⇒ empty.
- `appSuspendSet`: an always-available app survives a schedule lock; does not
  survive a stand-down; does **not** override a standing blocklist — and the
  same, newly, for `listeningExempt` at `locked = true`.
- Expiry resolved against the device clock, unchanged by a restart.
- LockTask allowlist re-derived when the clause changes and when an entry
  expires — including after a reboot.
- Out-of-hours use accrues to its own counter and **not** to the daily budget.

## Hardware gate (decented)

1. Set a schedule that closes now; name an audiobook app as standing. Wake the
   phone: the shade shows an **Open** row; tapping it opens the real app; set its
   sleep timer; audio stops on its own; the shade is back.
2. Confirm every other app is still suspended, and that leaving the named app
   returns to the shade rather than the launcher.
3. Add an app with an expiry a few minutes out. Confirm it opens before, and
   that after the expiry it is gone from the shade row and suspended again —
   without a restart.
4. Press **Finish now** while an always-available app is on the list. Confirm it
   is suspended and absent from the shade row — stand-down beats the clause.
5. Block a named app outright in Apps. Confirm it stays blocked at the lock.
6. Check the weekly picture and her ward mirror both show the out-of-hours line.
