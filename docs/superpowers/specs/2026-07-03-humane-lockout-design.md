# Charter for Linux — the humane lockout (design)

**Date:** 2026-07-03
**Status:** approved (decisions settled 2026-07-03); implementation pending — Phase A first
**Author:** lead engineer (Claude) + decented (founder / CEO-CTO)
**Context:** Found during today's T1 observe→freeze bring-up on real Mint hardware.
Freeze-only proved the *mechanism* (a child can be stopped), but the experience —
the child's whole session freezes to a dead screen with no way out — is **not
shippable**. This spec defines the real end-user experience.

## The problem (why freeze-only is not the product)

Today's freeze-only run froze the child's **entire user slice**: mouse dead,
keyboard dead, no explanation, no escape. Three concrete failures for a real
family:

1. **The child is trapped.** A dead-frozen screen offers nothing — they can't log
   out, shut down, or ask for more time. On a **shared** laptop this can strand
   the machine for a sibling who still has time.
2. **No warning = rage.** Apps stop dead mid-game with zero notice. That's the
   single most enraging possible behaviour for a kid, and it makes the parent look
   like they broke the computer.
3. **It looks broken, not governed.** A frozen desktop reads as a crash, not a
   deliberate, explained boundary.

## Requirements (the acceptance bar)

**Invariants — must always hold:**
- **Never trapped.** A locked child always has sanctioned ways out on screen:
  **log out**, **shut down**, and **ask for more time**. No dead ends.
- **Never a surprise.** Visible **warnings** precede every lockout; nothing stops
  dead mid-activity without a countdown.
- **Never bricks the shared box.** Enforcement is **per child / per session**. One
  child hitting their limit must not block a **sibling who still has time** from
  logging in and using the machine. The locked child can **log out** to free the
  seat.
- **Never escapable by the child** in ways that defeat the limit: the lock blocks
  access to their apps; only the *sanctioned* actions are available; the guardian's
  recovery rails still work.
- **Explained.** The lock clearly states *why* (out of time / past bedtime) and
  *when they're back* (e.g. "opens at 7:00am").

## Current state (what exists vs the gap)

| Piece | Today | Gap |
|---|---|---|
| Lock UI (`charter-lock`) | A fullscreen X11 override-redirect window that **grabs input and swallows it** — a passive wall, **no buttons**. | Must become an **interactive** panel: lockout message + **Log out / Shut down / Ask for more time**. |
| Warnings | Enforcer **computes** `Warn(Ten)`/`Warn(One)`; runtime **drops them**. | **Deliver** them to the child's screen + add a final short countdown. |
| Freeze target | The **whole user slice** (`user-<uid>.slice`) — freezes the compositor too → dead screen; a lock overlay can't draw. | Freeze the child's **apps** only, leaving the session's display alive so the lock renders + accepts input. |
| Lock launch | charterd resolves the session `DISPLAY`+`XAUTHORITY` and launches the lock (enforce mode only). | Prove the draw/grab on real Mint (earlier X11 auth error); wire the action buttons back to root-mediated logout/poweroff/request. |
| Shared machine | Per-uid enforcement exists; each child judged independently. | Verify a sibling with time logs in unblocked while another child is locked; wire the lock's **Log out** to free the seat. |

## Design

### 1. Freeze topology — pause apps, keep the session drawable
Freeze the child's **application slice** (their running apps), **not** the session
slice that hosts the display server / compositor / the lock overlay. This is the
core correctness fix behind today's dead screen: the game/browser truly pauses (no
progress, no audio, no network), while the screen stays live enough for a
root-owned lock to draw and receive input on top. The exact cgroup target is
distro-shaped (Cinnamon/Mint) and **verified on the freeze rung** — the reconcile
loop landed today already applies whatever target we choose, level-triggered.

### 2. The lock becomes an interactive panel (not a wall)
`charter-lock` keeps the fullscreen override-redirect window + input grab (so apps
underneath are unreachable), but renders a real panel:
- **Headline + reason:** "Time's up for today" / "Past bedtime" + when they're back
  ("Opens at 7:00am").
- **Actions (root-mediated — the lock runs as root and performs these on the
  child's behalf):**
  - **Log out** — ends the child's session (frees a shared machine for a sibling).
  - **Shut down** — powers off.
  - **Ask for more time** — files a request (see §5).
- Everything else stays swallowed by the grab. The child cannot reach their apps,
  but is never stuck.

### 3. Warnings + a soft countdown (no mid-game death)
- Deliver the enforcer's existing `Warn(Ten)`/`Warn(One)` to the child's session as
  on-screen notifications ("10 minutes left", "1 minute left").
- Add a final **on-screen countdown** in the last seconds before the lock draws, so
  the stop is expected, not abrupt.
- At zero, the lock **appears** (apps freeze underneath) with the explanation — the
  freeze is the enforcement, but the child sees *why*, not a mystery crash.

### 4. Shared machine — per-session, log-out-to-free
- Enforcement stays per-uid; a locked child's freeze + lock target **only their
  session/display**, never a sibling's.
- A sibling with remaining time logging in gets a normal, unfrozen session.
- The locked child's **Log out** action releases the seat so the sibling can use
  their time — this is the answer to "the first child locks the PC and they're
  stuck."

### 5. "Ask for more time" (Phase B — depends on the grant flow)
Two modes by pairing state:
- **Paired (guardian on phone):** the lock files a REQUEST that rides the relay to
  the guardian's MyCharter app (the approvals surface already exists); an approved
  GRANT flows back and unlocks/extends the child (the device already enacts grants).
- **Device-only (no phone):** an **on-the-spot parent approval** — the panel offers
  "Parent unlock", which takes the admin password/PIN and grants a bounded
  extension locally. (A parent is physically present on a device-only setup.)

### 6. Recovery rails (unchanged — already built)
`sudo systemctl stop charterd` (ExecStopPost thaws all + re-enables VT), the
graphical **Charter Recovery** tool (pause/thaw behind admin), the sole-admin
brick-guard, and the fail-safe self-heal on restart all remain. The humane lockout
is additive; the guardian's escape hatches are untouched.

## Phasing (each phase ships + hardware-gates on its own)

- **Phase A — device-local humane lockout (no phone dependency):** freeze topology
  fix + interactive lock (message + **Log out** + **Shut down**) + delivered
  warnings + soft countdown + shared-machine/per-session correctness. This alone
  removes the "trapped / bricked shared box / rage-quit" failures and is fully
  testable device-only. **Highest value, do first.**
- **Phase B — request time/access:** the lock's **Ask for more time** →
  paired-to-phone request/grant, and the device-only on-the-spot parent unlock.
  Builds on Phase A; carries the grant-flow dependency.

## Decisions (approved by decented, 2026-07-03)

1. **Warning cadence:** notifications at **10 min** and **1 min**, then a **~30-second
   on-screen countdown** before the lock.
2. **At-zero grace:** **none** — the 1-minute warning + countdown *is* the grace; at
   0 the lock draws. (Open-ended grace gets gamed.)
3. **Phase-A lock actions:** **Log out + Shut down**. "Ask for more time" is Phase B.
4. **Request-time (Phase B):** **both** modes — paired→phone request/grant, and
   device-only→on-the-spot parent PIN.
5. **Display stack:** **X11 / Cinnamon** (Mint default) for v1; **Wayland scoped out**
   for now.
6. **Power actions on the lock:** **Shut down + Suspend** (both).

## Testing

- **Pure/unit (headless):** warning-threshold emission + delivery mapping; lock
  panel action → intent mapping (logout/poweroff/request); per-session targeting;
  the freeze-target selection. Extends today's `charter-schedule` / `charterd`
  suites + the four CI gates.
- **Hardware gates (decented — the real validators, staged like today):**
  1. **Freeze topology:** at lockout the child's apps pause **but the screen stays
     drawable** (not a dead session). 2. **Interactive lock:** the panel renders on
     the child's display, **Log out** and **Shut down** work, input to apps is
     blocked. 3. **Warnings:** 10-/1-min + countdown actually show before the lock.
     4. **Shared machine:** while one child is locked, a sibling with time logs in
     and uses the box normally. 5. **(Phase B)** request → phone → grant → unlock.

## Sequencing
Phase A first (device-local, no phone dependency), hardware-gated rung-by-rung on
decented's Mint box exactly like today's observe→freeze bring-up. Phase B after A is
proven, reusing the grant flow. Enforce mode (`CHARTER_ENFORCE=enforce`) is the
target once the lock is humane — we do **not** ship the bare freeze-only dead-screen
to families.
