# One visual language across the family's screens — design record

**Date:** 2026-07-28 · **Status:** ward surfaces implemented; MyCharter pass outstanding

## The problem

Charter had two unrelated design languages. MyCharter and charter.signet.you speak
warm paper (`#f7f6f2`) with a wax-seal brand (`#8f2a24`), 18/12/999 radii and a
defined type scale. The ward's phone spoke dark navy (`#0B1021`) with slate-blue
text (`#7C89B8`), flat `#2C3A6E` slabs, and emoji as iconography. Nothing was
shared, so a parent's phone and a child's phone did not look like one product.

Emulator screenshots (2026-07-28) showed worse than the palette split:

- **Lock shade:** content collapsed into the bottom third with a dead void under
  the clock; "Ask for more time" — the ward's sanctioned exit — was a grey
  ALL-CAPS platform button clipped at the bottom edge.
- **Ward app:** it showed a child an **adb command**
  (`adb shell dpm set-device-owner ...`) and a 64-character hex wall as the
  "device code", against the design system's own rule: "Audience: a
  non-technical parent. No dev jargon, ever."

## Decisions

1. **One token set, two moods.** `ui/CharterTheme.kt` mirrors `theme.css`: same
   brand, radii, type scale, tap floor. PAPER for the ward's app; INK — a *warm*
   dark derived from the paper, not an unrelated blue — for the lock shade.
   Dark is right for a screen met at bedtime; a full-page white shade is a torch
   in a dark room and costs more OLED battery.
2. **The seal is used once per screen**, on the way forward: "Ask for more time"
   on the shade, "Pair with guardian" in the app. Everything else is outlined so
   the exit stays the loudest thing.
3. **Three zones on the shade** — clock top, message centred, actions anchored at
   the bottom within thumb reach — via two weighted springs that collapse when a
   full lifeline (five numbers + emergency + break-glass) needs the room.
4. **Real buttons.** `setBackgroundColor` REPLACES the background drawable, which
   is what flattened the corners and killed the ripple. Buttons are now rounded
   drawables with a pressed state, 48dp minimum, sentence case.
5. **The terminal is for the adult at the cable.** The provisioning command moves
   behind a "Set-up help" tap; the device code leads with a readable short form
   and keeps the full value selectable beneath it.
6. **Behaviour untouched.** Insets/cutout handling, LockTask, the End-call bar,
   hold-to-call, break-glass and every string from the core are unchanged — they
   exist because of real incidents and are out of scope for polish.

## Verification

Emulator (`charter-ci`), before/after per screen; Device Owner set so the shade
renders as a ward sees it (LockTask hides the status bar). The app's own
`DISALLOW_INSTALL_APPS` blocked re-installs once Device Owner was active —
enforcement behaving correctly; debug builds are `testOnly` so
`dpm remove-active-admin` unwinds it.

## Outstanding

- MyCharter's own light pass (it is the reference the others moved toward).
- Real-glass check on a phone; emulator rendering is not proof of on-metal type
  weight or colour.
