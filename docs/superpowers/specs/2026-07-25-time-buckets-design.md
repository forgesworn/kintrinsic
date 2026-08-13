# Time buckets — "Play is an hour a day" (design memo)

**Date:** 2026-07-25
**Status:** Greenlit by decented in the same breath as the ask ("build it… it may
be that we list Minecraft as the play bucket and play is restricted to one
hour… it needs to satisfy my immediate need, but also think forward for wider
use cases").
**Reads against:** `docs/CONSTITUTION.md` — companion-not-control; the family
owns the data; transparency is the invariant.

## The ask

Restrict Minecraft to one hour a day on Robin's laptop. decented's own
generalisation, which is the better model: don't limit *Minecraft*, limit
**Play** — a named bucket of apps with its own daily allowance, of which
Minecraft is a member.

## Why buckets rather than per-app budgets

1. **It's how parents already think.** "An hour of games" is the real
   sentence; "an hour of Minecraft, an hour of Roblox, an hour of Fortnite" is
   an accounting exercise that a child defeats by installing a fourth game.
2. **Per-app is the degenerate case.** A bucket containing exactly one app IS
   a per-app budget, so buckets satisfy the immediate need without a second
   mechanism.
3. **Charter already has one.** The `learning` clause literally says it
   "credits a separate *learning* bucket instead of draining the screen-time
   budget", and it already carries a `capMinutes`. Buckets are that idea, made
   general and given teeth. This is a generalisation, not a new subsystem.

## The model

A bucket is a named set of apps with a daily allowance:

```
Learning   free — never drains the day        (exists: the `learning` clause)
Play       60 min/day, then its apps stop     (new: the `buckets` clause)
<default>  everything else drains the day     (exists: `budget`)
```

Spending a bucket does **not** lock the device — it closes *that bucket*. The
ward keeps the rest of their day. That distinction is the whole companion
frame: a spent Play bucket is "games are done for today", not "your computer
is confiscated", and the child can still do their homework, message their mum
and finish what they were writing.

## Attribution: reuse, don't reinvent

`linux/crates/charterd/src/focus.rs` already answers "which bucket is the ward
in right now?" every tick, from the **foreground window**, on OS-side identity
only (exec path / flatpak id / launcher marker — window titles are
page-controlled and deliberately never consulted). `Bucket` grows from
`Learning | Screen` to `Learning | App(bucket_id) | Screen`; `classify` checks
bucket membership alongside learning apps. Everything else — the tick loop,
the day-keyed counters, the `/proc` sweep and SIGTERM that `appRules` already
uses to stop an app — is existing, tested machinery.

**Foreground, not merely running.** Minecraft sitting minimised must not burn
the allowance; this falls out of using the focus probe rather than a process
scan.

**Fail-closed stays fail-closed, but in the right direction.** Learning fails
*to Screen* (an attribution error must never make time free). A capped bucket
fails *to not-blocked* (an attribution error must never confiscate an app the
child is entitled to). Both preserve the same principle: an error never
silently helps the enforcement side.

## Wire

New clause kind `buckets`, **store key 10** (next free after `lifeline`=9),
replace-the-set like `learning` and `apprules`, per-(subject,kind) monotonic
`issuedAt` rollback protection.

```ts
interface AppBucket {
  /** [a-z0-9-] slug — stable id, used for the day counter. */
  id: string;
  /** Guardian-facing name the family uses: "Play". */
  label: string;
  /** Members: Android package id, or Linux exec path / flatpak id — the SAME
   *  identity vocabulary `appRules.pkg` already uses. */
  apps: string[];
  /** The bucket's daily allowance. */
  dailyMinutes: number;
}

interface GrantBuckets {
  v: 1;
  buckets: AppBucket[];
  /** Lift every bucket (nothing is capped). */
  paused?: boolean;
  /** IANA tz the day boundary is computed in — same discipline as `budget`. */
  tz: string;
  issuedAt: number;
}
```

Validation is fail-closed on both sides: slug ids, non-empty label, at least
one app, `dailyMinutes` in 1..=1440, known tz. A malformed body caps nothing
(never over-blocks on a garbage clause) — matching `appRules`' posture.

## Per-device, for free

Bucket members are named with the same platform-specific identities as
`appRules` (a flatpak id or exec path on Linux; an Android package id on the
phone), so a Play bucket listing the laptop's Minecraft simply never matches
anything on the phone. decented's "specifically on his laptop" needs no
per-device setting — it is how app identity already works.

## Deliberately NOT in v1

- **Android enforcement.** The clause is platform-neutral and the phone will
  ignore a bucket whose apps it doesn't have. decented's stated need is the
  laptop; the phone is a second increment, not a blocker.
- **Site buckets.** `learning` handles sites via materialised app windows;
  capped web buckets ("an hour of YouTube") want that same machinery and are
  a follow-on.
- **Pooling a bucket across devices.** That is Half B's usage-sync (31115)
  applied to buckets, and should wait until the pooled device budget has had
  its hardware round.
- **Reporting per-app minutes to the guardian.** Enforcement needs the DEVICE
  to count locally; it does not need to tell the parent what was played. The
  "Add a game or app" dialog promises "never reports what was played" and this
  keeps that promise: STATUS carries per-BUCKET seconds ("Play: 45m of 60m"),
  never per-app. Per the per-device memo, per-app minute detail is exactly
  where reflection tips into monitoring.

## The bug this also fixes

`AddAppLimit` in MyCharter slugifies whatever the parent types into
`app_minecraft` and stores that as the rule's `pkg`. Enforcement matches a
flatpak id against the cgroup, or an exec path / binary name against the
running process — so `app_minecraft` can never match anything and the rule
silently does nothing. Per-app hours have been shipped-but-inert on Linux.
Buckets must pick apps by their REAL reported identity, and the existing
per-app card needs the same treatment.
