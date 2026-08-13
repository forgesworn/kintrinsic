# Approvals clarity: name the ward, decide once, dismiss the pile

**Date:** 2026-08-04
**Status:** approved (decented, 2026-08-04, with standing authority to make the
judgment calls — "I will just go with your recommendations")

Three complaints from real use, all on the guardian's side of a request.

## 1. "Your ward" names nobody

A break-glass notification reads *"Your ward opened their phone for 10
minutes."* With more than one ward, and more than one device each, that is
nearly content-free at the moment it matters most — the whole break-glass
bargain trades prevention for immediate transparency, and a notification that
cannot say **who** or **where** is not transparency.

Same for requests: *"Your ward asks for 30 more minutes"*.

**The obstacle.** The carrier app receives a gift-wrapped event and classifies
it to a verdict carrying `machine` — the device's pubkey (`classify.rs:42,68`).
It has no idea that pubkey is "Mia's Pixel 6", because names live in the
PWA (`Child.name`, `Device.label`) and the carrier is a separate Android app.

**The fix.** A roster, pushed PWA → carrier over the existing JS bridge
(`CarrierBridge`, which today exposes only `isCarrier()` and `provision()`).
The PWA already knows every `(devicePubkey → child name, device label)` pair;
it sends them, the carrier persists them beside its provision, and `Notifier`
looks up `machine` when composing text:

> **Emergency unlock used**
> Mia opened her phone (Pixel 6) for 10 minutes.

**When the roster has no entry, keep today's wording.** An unknown device must
read "Your ward", never a guessed name — see the standing rule against
fabricated user-facing values. The roster is a convenience, never a
correctness dependency: notifications must still fire, in full, with an empty
roster.

## 2. Deciding takes two taps, and the second one lies

Pressing **Approve** in the Approvals list opens the `SignSheet`, which asks
you to confirm. Pressing **Not now** opens the *same* sheet, whose call to
action reads **"Approve"** — so denying a request requires approving something.

The sheet is not wrong to exist. It carries real information when the signer is
external ("go to Signet and approve there") and it is a genuine safety net for
bulk rule edits, where "nothing changes until you confirm" is worth saying.

It carries nothing on a **decision** taken from a list that already shows the
request, the child, and the amount, under a button that already says exactly
what it will do.

**The fix, in two parts.**

- **A decision signed by the local key skips the sheet.** The Approve / Not now
  press *is* the confirmation. Rule changes (`action: "clause"`) keep it
  unchanged — a bulk edit and a one-tap answer are not the same act.
- **A decision signed externally still shows the sheet**, because the round trip
  to Signet is real and the guardian must be told to go there — but it must say
  the right thing. The gate's context grows from `"clause" | "decision"` to
  carry which decision it is, so a denial never renders the word "Approve".

## 3. No way to let a pile go

A ward who asks three times produces three rows. Answering each is the honest
default, but sometimes the right response to the third identical ask is simply
not to answer it.

**The fix.** A small ✕ on each request card. It removes the row from the
guardian's list and nothing else: no clause is signed, nothing crosses the
wire, the ward is not told.

**Why this does not break the transparency invariant.** Dismiss does not tell
the ward anything false — it leaves the request *unanswered*, which is already
what happens when a guardian does not act. The ward's own device shows "asked"
and lets her ask again once it goes stale (`ASK_STALE_SECS`, 20 minutes),
exactly as before. What changes is only that the guardian's list stops
accumulating. "Not now" remains the way to send a real answer, and stays the
visually louder control.

Because nothing is signed, dismiss needs no signer, no confirm, and works
offline.

**Available on every request, not only duplicates.** Gating it on "this is the
third ask" would be a rule the guardian has to learn, to save a control that
costs nothing.

## What this does not change

- The ward's side of an ask is untouched — no new message, no new state.
- `denyRequest` keeps its exact current behaviour and wording.
- Auto-sign, when on, already skipped the sheet; that path is unchanged.
