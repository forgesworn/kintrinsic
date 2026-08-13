# Multiple Devices — Rules, Reflection & the Family's Week (design memo)

**Date:** 2026-07-24 (reworked same day into the companion frame)
**Status:** **Half A LANDED 2026-07-25** (see the build order). Half B (the
family's week) is the HEADLINE — decented greenlit
implementation 2026-07-24 (only his family dogfoods, so this is the moment to
lay the foundation right). Half A (per-device rules) is a smaller parallel win.
**B1 (the weekly picture) LANDED 2026-07-24** — `apps/charter-app/src/insights/`,
rendered on the Activity screen. B2–B4 open.
**Reads against:** `docs/CONSTITUTION.md` (companion-not-control; family owns the
data; transparency is the invariant; Charter aims to make itself unnecessary).
**Trigger:** decented paired Robin's phone alongside the laptop; both land under
one child sharing one charter. Two questions fell out: how should rules span a
child's devices, and — the bigger one — how does Charter help a family *see and
shape* screen time over time, as a companion rather than a controller.

This memo has two linked halves. **Half A** (the rules model) is mostly settled
and small. **Half B** (reflection & the week) is the larger idea and the reason
this is interesting.

---

## The frame (why this isn't an enforcement problem)

The instinct on "one child, many devices" is to worry about a child hopping
devices to get more time, and to reach for a hard cross-device cap. Per the
constitution, that's the wrong frame. A device-hop isn't a breach to seal — it's
*visible*, and a conversation. Charter's job is not an un-gameable cage; it's a
**legible agreement** a family can hold in their head, plus an honest **picture**
they can reflect on. Everything below follows from that.

---

# Half A — The rules model

## What the incumbents do (verified research, 2026-07-24)

Deep-research fan-out (104 agents; 22/25 claims confirmed by 3-vote adversarial
verification). The facts, with the companion-frame reading:

| System | Daily screen-time budget | Per-app limits | UI |
|---|---|---|---|
| **Google Family Link** | **Per-device** — official help: a 2-hour limit gives 2 hours on *each* device | rules propagate to all devices | per-device screens |
| **Apple Screen Time** | **Per-device**, even with "Share Across Devices" — that copies *one ruleset* to each device and aggregates usage in the dashboard; 2h phone + 2h iPad = 4h. Syncing one policy is the intended design (iOS 16.5 fixed a sync bug). | per-device | one synced ruleset; combined usage shown |
| **Microsoft Family Safety** | **Per-device / per-platform** — Windows / Xbox / Mobile(=Android) tabs; can't split two devices of the same platform (ties to the account). | combined quota across devices | platform tabs |
| **Norton Family** | **Per-device**, per-device granularity | — | per-device |
| Qustodio / Bark / Kaspersky / Canopy / mmguardian | *no evidence survived verification* | — | — |

**The honest reading:** per-device is universal, and "pooling" the overall budget
is essentially mythical (even Apple doesn't). We don't need to match them for
control reasons — we should land per-device because it's **simple and legible**,
and because **schedule already does the "total time" job better** (below). The
one place cross-device *pooling* is real is per-app limits (Google, Microsoft) —
and, tellingly, that needs usage aggregation, which is exactly Half B.

Sources: support.google.com/families/answer/7103340; Apple Support Communities
253796695 / 250997911 / 255511715 + iOS 16.5 sync-bug (TechCrunch 2023-07-31);
Microsoft "Set screen time limits across devices" docs + learn.microsoft.com Q&A
3886145.

## What Charter is today (grounded in the code)

- `Child` has `devices: Device[]` **and** `policies: Policy[]`. A `Policy` scopes
  to `{kind:"device"}` (the whole-computer charter) — **not to a specific device.**
- **Transport is already per-device.** `realSigner` resolves a child to
  `devicePubkeys[]` and *"delivers every clause to every device — one wrap per
  device."* Each device has its own key and clause subscription.
- **The warden is already per-device.** Each device sees only the clauses wrapped
  to *it*, keyed per-`(subject, kind)`, and counts its own usage locally. It has
  no concept of the child's other devices.
- A clause carries `subject` (the child) for attribution; there is **no device
  field and none is needed** for per-device rules.

**So per-device rules are a PWA-only change** — author device-specific clauses and
wrap each device its own, instead of the same one to all. No wire or warden change.

A quiet honesty bonus: Robin's two devices are *different platforms* (Linux
laptop + Android phone). Some controls are already device-specific — the hotspot
and the lock-screen lifeline are Android-only; app package names differ. "Fully
combined" was never quite truthful; surfacing per-device is more honest, not more
controlling.

## Recommended model: "one charter, split where it fits the child"

One **base charter** (what you author today), applied to every device. For any
control you can say **"set separately for this device"** — an override for that
control only. Combined by default; diverge where a real child's life calls for it
(the homework laptop genuinely differs from the fun phone — that's fitting a
family, not tightening a net).

**Table UX** (shown only when a child has 2+ devices — a one-device family never
sees any of this):

```
 CONTROL       │  🖥 Laptop (Linux)   │  📱 Phone (Android)
 ──────────────┼──────────────────────┼─────────────────────
 Daily time    │        2h  ◀── shared ──▶
 Schedule      │  Mon–Fri 4–8pm       │  Every day 7am–9pm   ← split
 Websites      │        older · SafeSearch  ◀── shared ──▶
 Apps          │        block: none  ◀── shared ──▶
 Hotspot       │        — (n/a)       │  Filtered            ← phone-only
 Lifeline      │        — (n/a)       │  Mum, Dad            ← phone-only
 Learning      │        Khan free  ◀── shared ──▶
```

**Split at CONTROL granularity** (the line item), not whole-child profiles and not
sub-fields — it maps onto our existing independent clauses and keeps the model
legible. Sensible defaults: schedule / budget-value / websites / apps / learning
default **shared**; hotspot and lifeline are **per-device by nature** (only a phone
has them) and render only on capable devices.

**Adding a second device inherits the child's charter** (Apple's default; least
surprise), with a gentle "these now also cover [device] — want anything different
for it?" nudge. Never a blank re-configuration.

**Data model (PWA-only):** keep the child-level `Policy` as the base; add
per-device overrides (smallest diff: `deviceOverrides?: Record<deviceId,
PolicyControlSubset>`). Publish path changes from *same clause to every device* to
*each device gets its effective clause* (base + its overrides). **Migration:**
existing children → every control "shared," byte-identical to today.

---

# Half B — Reflection & the family's week (the bigger idea)

This is where Charter stops being a rule-setter and becomes a **companion**: it
helps a family *see* how screen time is going and *shape* it toward a goal —
like a smartwatch that builds a habit and then becomes irrelevant. Per the
constitution, this is **reflection, not policing**, and the data is the family's.

## The reframe that makes "pooling" easy and right

Earlier thinking said "don't build pooled budgets — real-time cross-device
enforcement fails when a device is offline." That was the control frame, and it's
the wrong problem. In the companion frame we don't need a device to *enforce* a
shared count in real time — we need the week to **reconcile** once devices are
back online.

- **Eventual reconciliation, not real-time enforcement.** Each device already
  counts locally (offline is fine). On reconnect it publishes its usage summary
  to the guardian — exactly the **reserved `USAGE_SYNC` (kind 31115)**. MyCharter
  aggregates per child across devices. **Usage-sync is the backbone of Half B
  and worth building** (this flipped the earlier "don't build it" conclusion).

**decented's call (2026-07-24): the pooled budget is BOTH displayed AND enforced.**
The earlier "pooling-as-understanding yes, pooling-as-policing no" split is
superseded: the pooled number becomes the number the existing budget enforcement
uses. Mechanically (per the frozen `spec/contract.md` §USAGE_SYNC model — the
guardian authority is the aggregator, so there is **no new trust root**): each
warden reports its usage; the guardian side consolidates per child and sends
each device a guardian-signed "elsewhere" view; each device enforces the pooled
budget against its own minutes ∪ that freshest elsewhere view. An offline
device can't see the pool — it keeps counting and enforcing on what it knows,
and on reconnect any true overage surfaces as the overdraft on the week, never
silently dropped. Staleness only ever *under*-counts the pool, so a flaky
relay can't cause a wrongful early lock.

## Counting time honestly: the union rule (no double-counting)

Simultaneous use — gaming on the laptop while chatting on the phone — must
count **once**. So usage is not a per-device duration to sum but a per-day
**set of active minutes** per device (a 1440-bit bitmap, ~180 bytes/day; just
"screen active this minute" — no apps, no content; family-owned and
ward-visible like everything else). The child's day usage = |union of their
devices' sets|. This is what the B3 journal stores and what USAGE_SYNC carries;
the scalar `usedTodaySecs` in STATUS can't express it, so **B1's chart (which
sums scalars) can slightly overcount simultaneous use until B3 lands** — a
known, temporary, honestly-stated gap. A side bonus: the overlap is itself
visible ("both at once" minutes), which is an honest thing to show a family.

## The overdraft model

A hard cap must make an ugly binary choice offline (block or allow). An overdraft
doesn't:

- The pooled budget is **enforced** the way today's per-device budget is —
  against the best pool each device can see (own minutes ∪ freshest sibling
  reports), degrading gracefully to local-only knowledge when offline.
- The **pool reconciled after the fact** across the child's devices: whatever
  an offline device couldn't know at the time is still logged and shows up.
- **Going over shows as the red part of the week's bar** — to reflect on and wind
  back. It might be gaming, a YouTube rabbit hole, or simply losing track of time
  the way anyone loses track of money some months. The overdraft is never
  silently dropped and never retroactively punished — it's the visible,
  talk-about-it part of the week.

## The weekly picture — concrete sketch (increment 1)

The first visible piece. A calm weekly bar chart of the child's screen time,
one bar per day, per device, against the agreed daily allowance — the overdraft
shown, not slammed.

```
  Robin · this week                         [ Guardian view ▾ ]

   3h ┤                                  ▓▓
      │                        ▒▒        ██   ← ██ over the agreed time
   2h ┤ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ▓▓ ─ ─ ─ ─██─   ← agreed: 2h/day (dashed)
      │              ▒▒        ▓▓         ▓▓
   1h ┤  ▓▓    ▓▓    ▓▓   ▓▓   ▓▓    ▓▓   ▓▓
      │  ▓▓    ▓▓    ▓▓   ▓▓   ▓▓    ▓▓   ▓▓
      └────────────────────────────────────
        Mon   Tue   Wed  Thu  Fri   Sat  Sun

   ▒ Laptop   ▓ Phone   ██ over the agreed time
   ─────────────────────────────────────────
   This week: 12h 40m · 2 days over · a little down from last week
```

- **One bar per day**, stacked by device (so you see the phone-vs-laptop split),
  over the last 7 days ending today.
- **A dashed line at the agreed daily allowance** (the child's budget). The part
  of a bar *above* the line is the **overdraft**, in a warm — not alarming — red.
  Reflect and wind back; never a locked drawer.
- **A one-line reflective summary** underneath (total, days over, gentle trend) —
  no scores, no streaks, no dopamine mechanics. Constitution guard: this is the
  place a companion product could quietly become an engagement product; it must
  stay calm and reflective, and never nag.
- **View toggle** seam: Guardian view now; **the ward's own view** is the next
  increment (they see their own week — the smartwatch model).

**Data (increment 1):** MyCharter already receives device STATUS heartbeats
carrying `dayKey` + `usedTodaySecs` per device; we persist a per-device,
per-day usage history (max observed per day) and render the week from it. This
is *live-derived* and honest about its one gap — days the app never polled can
undercount — which the device-journaled **usage-sync (kind 31115)** backfill
closes later. The history's shape is identical either way, so nothing here is
throwaway.

## The plan (Charter as a journey, not a limit)

The insight layer turns rules into a **plan**: "ease from 6h/day toward 1h/day
over eight weeks," because cold-turkey isn't right for every child or every
parent. Progress is something the family — **and the ward** — can watch, the way
you watch steps on a watch. The best outcome is the plan completing and Charter
being switched off (constitution §4: graduation is a celebrated, proud exit).

## Where the data lives and who sees it

- **The family's, full stop** — on-device, no cloud (already true by
  architecture). What they collect, read, and share (e.g. with a psychologist) is
  their call.
- **The ward sees their own trends** by default (self-reflection, the smartwatch
  model), subject to the constitution's §3 open call on ward-visibility default.
- **Transparency is the invariant; we are never the covert party.** Tracking is a
  choice a family turns toward — and away from when it's done its job.

## Reconciling with "enforce, don't surveil" (a prior decision, consciously revisited)

We previously said *no per-app usage reporting* ("Sam played ExampleGame 40 min") to
avoid the surveillance vibe. Half B revisits that under the constitution: usage
*reflection* is legitimate when it's the **family's own data, transparent, and
ward-visible**. The prior stance was really guarding against *covert, company-side
monitoring* — which the constitution forbids anyway. Guidance: aggregate and
category-level trends and the wind-down plan are clearly reflective; per-app,
minute-level detail is where "insight" can tip toward "monitoring," so keep it
deliberate and ward-visible-first. This supersedes the blanket no-reporting stance
where they conflict; it does **not** loosen the covert-monitoring prohibition.

---

## Build order (Half B is the headline; Half A is a parallel small win)

**B1 — the weekly picture in MyCharter (LANDED 2026-07-24).** Persist a per-device,
per-day usage history from the STATUS heartbeats MyCharter already receives;
render the calm weekly bar chart with the overdraft against the agreed
allowance; the reflective one-liner. PWA-only, no wire change. This ships the
visible differentiator on real dogfood data immediately.

**B2 — the ward's own view.** Mirror the weekly picture to the ward device
(their own week — the smartwatch model). Constitution §3 ward-visibility default.

**B3 — device-journaled usage-sync (kind 31115) + pooled enforcement.
(B3a + B3b LANDED 2026-07-24: minute journals + STATUS bitmap + 31115
verify/ingest + pooled union enforcement in BOTH wardens' Rust, AND the
MyCharter aggregator — per-device elsewhere views published change-gated,
union weekly chart with the "both at once" segment. Left: Android
.so/APK rebuild + the bundled two-device hardware round.)** Each
warden (Linux + Android) journals per-minute activity bitmaps and reports them
on reconnect; the guardian side consolidates and returns each device a
guardian-signed **union-of-elsewhere** bitmap (contract §USAGE_SYNC, extended
with the union rule — simultaneous use counts once). Wardens enforce the pooled
budget against |own ∪ elsewhere| (offline → local knowledge, reconciled as
overdraft), and MyCharter's chart switches from summed scalars to union totals
(with the "both at once" overlap attributable). Closes B1's two gaps at once:
offline-complete history and double-counted simultaneous use.

**B4 — the plan & graduation.** Wind-down goals (ease NNh/day → n over weeks),
progress the family (and ward) can watch, a celebrated graduation. Optional
pooled *per-app* limits become possible here, as an explicit family choice,
never the default.

**A (parallel, independent) — per-device rules. LANDED 2026-07-25.** The Half-A
model above, built as designed: `deviceOverrides` on the device-scope Policy
(keyed by `Device.id`), split at control granularity, shared by default, the
affordance gated behind 2+ paired devices. Splitting seeds every device with
the current value so it changes nothing until a parent edits one; re-sharing
the last split control drops the map entirely, leaving a never-split child
byte-identical to before the feature existed. Publishing resolves base +
overrides into one effective policy per device — and `deviceOverrides` is
stripped unconditionally on the way out, so no device can ever read its
sibling's rules (asserted directly in `realSigner.test.ts`). PWA-only as
predicted: no wire or warden change.

Two things the build surfaced that the design didn't call out:
- A save signs only CHANGED dimensions, so a split control has to be signed
  even when the shared value is untouched — otherwise a parent edits the
  phone's limit, hits Save, and it silently goes nowhere. `pickOverrides`
  narrows the map to exactly the dimensions being signed, so an unchanged
  control can't be grafted back on and re-signed stale (the dual-dimension
  save-revert bug in a new hat).
- `ChildTarget` now carries devices as `{id, pubkey}` rather than a bare key
  list — per-device rules need to know WHICH device a wrap is sealed for.

Deferred deliberately: **inherit-on-add with the "these now also cover
[device] — want anything different?" nudge** is NOT built. Adding a device
today inherits the base charter by default (which is the right behaviour and
the least surprising), but there is no prompt.

## Open questions for decented

**Q1 and Q4 were answered by building** (2026-07-25, decented: "fix, build
everything you can"): Half A shipped on the memo's recommended defaults —
combined-by-default, split per control, websites SHARED by default like the
rest. Both are one-line changes if he wants them different. **Q2 and Q3 are
still genuinely open.**

1. ~~Half A model — "one charter, split per control," combined-by-default —
   agree?~~ **Built on that model 2026-07-25.**
2. Ward-visibility default (constitution §3 open call) — where do you set it?
3. Phase 2 scope for a first cut — is the **weekly picture + wind-down plan** the
   right headline, with pooled per-app limits deferred until asked for?
4. ~~Any control whose Half-A default should flip (e.g. should *websites* ever
   default per-device — a laptop browses more than a phone)?~~ **Shipped
   shared, like every other control. Flipping websites to default-split is a
   one-line change if a real family wants it.**
