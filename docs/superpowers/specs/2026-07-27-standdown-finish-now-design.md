# Stand-down — "Finish now" (design)

**Date:** 2026-07-27
**Status:** Android **proven on hardware 2026-07-27**; Linux (charterd 0.4.0) code-complete, not yet run on a laptop
**Contract:** `spec/contract.md` → *Stand-down — "finish up now"* (clause 13)

## The ask

decented, 2026-07-27:

> there is a give time option for each child, which works and is good. i would
> like to be able to give them a 1 min warning, then lock them out for the rest
> of the day, or until allowed back on.

The inverse of Give time (the `gift` clause), which adds minutes on the
guardian's initiative. This takes them away on the same initiative.

## Decisions taken

Two forks were genuinely the product owner's, and were put to him:

1. **How does the ward come back?** → *Locked until the guardian lifts it,
   lapsing at her midnight.* Give time does **not** quietly undo it; lifting is
   a deliberate act. A forgotten stand-down must not become a permanent one.
2. **What still works while locked?** → *The same as running out of time.*
   Learning apps stay available (they are deliberately time-free), and the
   lifeline is non-negotiable either way. Bedtime is what the daily schedule is
   for.

Decision 2 is what kept the design small, and it is also the safety argument:
reusing the existing lock means the lifeline, the in-call passthrough and
break-glass keep working with no new code path to get wrong.

## Mechanism

A stand-down **caps the ward's remaining time at the grace she is owed and then
holds it at zero.** It is not a second kind of lock.

Everything else is machinery that already existed:

| Need | What provides it |
|---|---|
| The 1-minute warning | `WarnLevel::One`, already fired at 60s, already reaching the ward in any app |
| Her seeing the minute | the time-left widget counts 60 → 0 |
| The lock | the ordinary shade, with the learning exemption and lifeline intact |
| Ordering vs a gift | the cap is applied **after** the extension pools |
| Rollback safety | the per-(subject, kind) monotonic `issuedAt` floor |

`LockReason::StandDown` exists purely so the ward is told *which* wall she met.
A spent budget waits for tomorrow and a closed window waits for the window, but
a stand-down waits for a **person** — and an unexplained lock mid-evening reads
as the phone breaking.

## Two choices worth defending

**It fails OPEN**, the mirror of the gift's fail-CLOSED. The governing principle
is that doubt must never invent a state harsher *or* looser than what the
guardian signed; both land on the standing charter. This direction matters more:
a malformed clause that locks a ward out of her own phone while her guardian
believes she is fine is worse than one that leaves her alone, and the guardian
can see it did not apply and try again.

**The grace clock is the device's.** The lock instant is deliberately not on the
wire. Stamped as `issuedAt + graceSecs`, relay transit would eat into the
warning, and a phone that spent an hour offline would reconnect and cut her off
with none at all — the "answer nobody sees" failure this codebase already
refuses. The device pins first-sight of the clause `id`, durably (slot 101),
because an in-memory pin would restart the grace on every boot and the lock
would never land.

## Bugs found on the way

- **Double warning.** Any drop past both thresholds in one tick armed both, so
  the ward would hear "10 minutes left" and "1 minute left" together. A
  stand-down does it every time; a cross-device usage sync could already.
- **Unknown `lockReason` binned the whole STATUS.** MyCharter returned null, so
  a phone on newer firmware would vanish from the guardian's view and read as a
  dead phone. That made every future clause kind a deployment trap, since
  devices self-update ahead of the guardian's app. Now degrades; the rule is
  written into the contract.

## Verification

Code: 410 core tests (+11), 184 linux, 324 PWA (+3); clippy and fmt clean in
`core/`, `linux/` and `android/jni`; PWA typecheck and build clean.

**Hardware: the Android happy path is proven.** 2026-07-27, on a real ward's
phone: "Finish now" delivered the warning and then the lockout.

Two things that round exposed:

- **MyCharter contradicted the lock.** The ward card summarised a child by the
  device that had reported most recently, so a second device's unlocked answer
  overwrote the locked one — "Allowed now — live" beside a locked ward. Fixed:
  a fresh locked report now speaks for the ward.
- **`charterd` did not enforce it.** The spine's enforcer inputs carried
  `stand_down: None` — a compile stub I never came back to — so a Linux ward
  ignored the clause and kept reporting allowed. That is almost certainly the
  device that overwrote the card. **Fixed in charterd 0.4.0**, with the grace
  rule shared rather than reimplemented.

Still unproven, in rough priority:

1. **The lifeline still dials while stand-down-locked** — the one that matters.
2. Give time does NOT lift it; "Allow back on" does.
3. It lapses by itself at the ward's midnight.
4. A reboot mid-grace does not restart the minute (slot 101 doing its job).
5. Linux: charterd 0.4.0 is published — run the same round on a laptop.
