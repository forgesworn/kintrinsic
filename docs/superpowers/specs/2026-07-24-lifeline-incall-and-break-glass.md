# Lifeline, the in-call trap, and the break-glass override (design memo)

**Date:** 2026-07-24 (evening — designed the same night the flaw was found)
**Status:** **ALL THREE PARTS IMPLEMENTED 2026-07-24/25.** In-call passthrough
+ End-call bar shipped in ward 0.3.1; lifeline v2 (5 numbers, platform
emergency entry, hold-to-call friction) and the break-glass override shipped
in ward 0.3.2 (versionCode 14) + carrier 0.1.2. Guardian knobs are live in
MyCharter behind a self-lifting ship-order guard. **Hardware verification is
the one thing outstanding** (see the round below).
**Reads against:** `docs/CONSTITUTION.md` — this memo is the constitution
applied to telephony: no technical barrier where safety or trust matters;
transparency instead of prevention.

## The incident (found live, 2026-07-24 ~20:38)

The ward tapped the lock-screen lifeline. The call dialed (mic indicator up),
but the **lock re-asserts itself every tick and shoved the system in-call UI
back underneath** — the ward was live on a call with no controls. The guardian
rejected the call; it rolled to voicemail; the ward had **no way to hang up**,
through and past the voicemail recording. The only escape was granting time to
unlock the phone. A pocket-dial variant records a guardian's voicemail at 2am
with nobody able to stop it. **Verify next hardware round:** whether the
overlay does the same to genuine emergency dialing (999/112) — if so this is
not UX, it is a safety liability.

## Principle: the lock is a SHADE, not the enforcement

Enforcement on Android is package suspension underneath; the overlay is only
what's at the glass. Therefore:

1. **In-call passthrough (the urgent fix).** While telephony is active (any
   call state, in or out), the shade stands down and the system in-call UI
   owns the screen; the shade re-fronts when the call ends. Belt-and-braces:
   an **End call** bar on the shade itself. No loophole opens — everything
   else stays suspended; the only usable thing is the phone being a phone.
2. **The dialer is never suspended.** "Phone-only unlock" is not a mode we
   build; the shade simply treats the dialer/contacts/in-call as
   always-allowed foregrounds and lifts while one is in front. A ward
   "hiding" in the dialer is making phone calls — in the companion frame
   that is fine, and it is visible.

## Lifeline v2 (decented's parameters, 2026-07-24)

- **Up to 5 numbers** (was 1–3), plus an **optional emergency-services
  entry** — **region-correct automatically**: sourced from the platform's
  emergency-number list (`TelephonyManager.getEmergencyNumberList()`,
  SIM/network-aware — 999 in the UK, 911 in the US, 112 in the EU, 000 in
  AU…), never a hand-maintained table and never a typed-in number. The
  family only toggles whether it shows; the ward sees the number their
  country actually answers. Guardian numbers each carry the warm label the
  ward knows ("Dad").
- **Wire order matters:** device-side validation is fail-closed, so a
  5-number clause sent to an old device renders NO lifeline. Ship acceptance
  (1..5) to devices FIRST (self-update is proven now), then let MyCharter
  author up to 5.
- Calls **to lifeline numbers are always allowed and quietly logged** — they
  are not "the override", they are just the phone working for the people who
  matter. Genuine emergency dialing sits outside everything, unconditionally.

## Anti-pocket-dial friction (required)

Deliberateness without literacy or PINs (a locked screen in a pocket must
never dial; a panicked 8-year-old must still manage it):

1. **Hold-to-call**: press and hold the number's button ~2s with a filling
   ring + haptic; releasing early cancels.
2. **Cancel window**: after the hold completes, "Calling Dad in 3… 2… 1 —
   tap to cancel" before the dial actually fires.
3. The emergency-services entry gets the same two gates with a longer hold —
   a false 999 call is its own harm.

## The break-glass override (the feature)

The fire-alarm model: **nothing stops you breaking the glass; breaking it is
loud.** One deliberate action on the shade (hold-to-activate + a warm
confirmation stating exactly what happens): the phone unlocks NOW — no
approval wait, works OFFLINE (safety cannot depend on the relay) — and
simultaneously it is journaled in Activity, the guardian is notified the
moment the relay allows ("Sam used the emergency unlock at 20:41"), and it
shows in the week.

Family-set knobs (matching where the child is on the journey):
- **Scope:** calls-only vs the whole phone.
- **Duration:** re-shades after N minutes (default ~10) unless extended.
- **Availability:** on/off per child.

Deliberately **no rate limits and no cooldowns** — a technical cap on an
emergency button rebuilds the cage. Overuse is a conversation, and the
transparency is what makes the conversation happen. The child knowing "I can
always do this, and Dad will know I did" is the trust model in one sentence.

## Build order (when green-lit)

1. **P0 — in-call passthrough + End-call bar + verify 999 path** (the trap).
2. Lifeline v2: device-side 1..5 acceptance → MyCharter authoring 5 +
   optional emergency entry; hold-to-call + cancel-window friction.
3. Break-glass override: shade affordance + offline journal + carrier
   notification + Activity entry + the three family knobs.


---

## Hardware round (outstanding)

With a paired phone on ward 0.3.2 and the guardian on carrier 0.1.2:

1. **In-call passthrough** — lock the phone, lifeline-call the guardian,
   REJECT it: the in-call UI must be usable over the shade and the call
   endable (either the system's hang-up or the shade's red End-call bar).
   This is the trap from 2026-07-24; it must be gone.
2. **999 path** — with the emergency entry enabled, confirm the shade's
   emergency button shows the LOCAL number and that dialing it is not
   trapped. (Use with care / a test SIM if possible.)
3. **Friction** — a quick tap does nothing; a 2s hold then the 3-2-1 window
   places the call; tapping during the countdown cancels it.
4. **Break-glass** — enable it in MyCharter (needs every phone on code ≥14
   for the controls to appear), hold the shade's emergency unlock: the phone
   opens immediately, the guardian's carrier raises "Emergency unlock used"
   within a poll, Activity shows it, and the phone re-locks at expiry.
   Repeat with the phone in airplane mode: the unlock must still be instant
   and the notification must arrive once connectivity returns.
5. **Vital signs** (ward 0.3.3, added 2026-07-25) — the shade now carries its
   own clock, battery gauge and reach indicators, because LockTask runs at
   `LOCK_TASK_FEATURE_NONE` and takes the system status bar with it. Check on
   the glass: the clock is correct and ticks; nothing sits under the Pixel's
   camera cutout; the battery matches reality (and shows a bolt on charge);
   Wi-Fi and mobile chips match the radios. The airplane-mode run in step 4
   doubles as the check that the strip says **"Airplane mode"** rather than
   showing a healthy mobile chip — keying reach off SIM presence made exactly
   that mistake, and it was fixed before shipping.

   Emulator-verified already (real screenshots, ward 0.3.3): paint, cutout
   inset, 12% red, charging bolt, airplane-mode label, Wi-Fi/mobile chips.
   What only real glass can settle: legibility at arm's length for a child,
   and the cutout on the actual 4a 5G.
