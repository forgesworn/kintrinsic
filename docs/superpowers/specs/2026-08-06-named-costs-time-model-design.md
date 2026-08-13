# Named costs — one time model, three tiers

**Date:** 2026-08-06
**Status:** design agreed with decented, building
**Origin:** decented, after finding that a dual-screen setup makes the current
free-time grant read zero while YouTube plays on the second monitor.

---

## 1. The problem, in one scene

Rob has two screens. A game runs full-screen on one; YouTube plays on the
other. He puts Khan Academy in front on the first screen.

Charter probes the focused window, sees a sanctioned Khan window, and credits
**Learning**. His screen budget does not move. YouTube costs nothing. The game
costs nothing. He can watch YouTube all evening for free without doing anything
Charter would recognise as a bypass — just by leaving a maths window in front.

That is not a bug in the free-time grant. It is the free-time grant working
exactly as specified, on a machine whose shape the specification did not
anticipate.

## 2. Why the current model produces this

Three mechanisms run at once on top of a charge-by-default baseline:

1. **The baseline.** Whoever holds the active login session is charged every
   second, whether or not anything is open.
2. **Learning *waives* a second.** A sanctioned pinned site app in focus means
   that second never touches the budget.
3. **Named times *charge alongside*.** A counted group debits its own allowance
   *and* the screen budget.

Two of those pull in opposite directions against a default that already
charges, and all three are adjudicated from **one probe of the single focused
window**. The interactions are specified nowhere except in the enforcer's
source.

**The waiver is also where every serious bug has been.** Forged `--class`
markers beside fake resolver pins; a `flatpak install --user` impostor of a
real app id; a bind-mounted binary making a ward's own file read as root-owned.
The hardening that answers those — reading ownership off the kernel's magic
symlink, exact argv multiset matching, refusing to trust anything the ward can
write — exists to protect one thing: the grant of free time.

Waiving is the only mechanism in the system that creates time out of nothing.
It is therefore the only one worth attacking.

## 3. The model

**Free stops being something Charter grants and becomes the absence of anything
that costs.** You cannot forge an absence.

A child's charter selects exactly one **time model**:

| Tier | The sentence a parent says | Mechanism |
|---|---|---|
| **Hours** | "The laptop works between seven and eight." | Schedule only, no budget. |
| **Session** | "Two hours a day." | Today's model, unchanged. Active session costs. |
| **Named** | "The laptop is free. YouTube and Minecraft cost." | Only named apps cost. |

Tiers are **exclusive**. Choosing one means giving up the others' behaviour, and
the guardian app says so at the point of choosing. Most of the gray area decented
described comes from all three being simultaneously live today.

Access rules — blocked, ask-first, app holds, always-available — are orthogonal.
They are about *whether*, not *how long*, and apply in every tier unchanged.

### 3.1 What "costs" means on a computer

**A costing app costs while it has a window open.** Any screen, any virtual
desktop, focused or not, minimised or not. It stops when the window closes.

Not focus: focus is a decent proxy for attention on one display and a bad one
on two, and it is what produced §1.

Not visibility: occlusion, monitor geometry and stacking order are a large
amount of fiddly code, a large amount of hardware-test surface, and a large
number of ways around it (another workspace, a covering window, audio only).

Not process liveness: this is the WhatsApp objection, and it is decisive.
Messaging and chat clients are resident by design — that is how messages
arrive. Under a naive "running costs" rule a child is charged all day for a
chat app opened once at breakfast, with no way to see why.

**Windows separate these cleanly, by a happy accident of how tray apps behave.**
An app that "minimises to tray" — WhatsApp, Discord, Steam, Slack, Spotify,
Element — *destroys* its window rather than hiding it. The process lives on; the
window manager no longer lists it. A tray-resident chat app therefore falls out
of a window-based rule for free: no allowlist of background apps to ignore, no
special-casing, no maintenance.

What it still catches is everything that matters: a full-screen game on the
second monitor, YouTube on the second monitor, anything parked on another
virtual desktop, anything minimised to the taskbar.

**A minimised window still costs.** Deliberate. Audio keeps playing when YouTube
is minimised, it is one click from being back, and "hide it to stop the clock"
is the wrong lesson. *Close it, don't hide it* is a habit a nine-year-old can
act on.

### 3.2 Counting once

Wall-clock time is not duplicable. In `named` mode, for each tick:

- If **at least one** costing app has a window open, the day and week budget are
  charged **once** for the elapsed seconds.
- Every **distinct named allowance** represented by those windows is debited
  once for the same seconds (deduplicated by group id — two members of Play
  open together spend Play once).
- If **no** costing app has a window open, nothing is charged at all.

An idle desktop costs nothing. Khan alone costs nothing. Khan plus YouTube costs
— because YouTube costs, not because Khan stopped being free.

### 3.3 What "costs" means on a phone

Nothing to decide. A phone shows one app at a time and the OS reports which one
is in front, which is what the Android ward already reads. The
running-versus-on-screen question does not arise. The family-facing sentence is
identical on both devices: **what you're using costs.**

The Android delta is genuinely small, because the ward already accrues only
when something is foreground *and* the screen is interactive
(`WardenController.kt:331-332`, `android/jni/src/warden.rs:1846-1853`). The
window set on a phone is simply the foreground package, of size zero or one, so
`named` mode there is one condition on an existing branch: charge only if that
package is a costing identity.

Both platforms also already reuse the same evaluation core — Kotlin decides
*how* to apply a suspension, never *what* to suspend
(`android/jni/Cargo.toml:34-42`) — so the model lands once, in
`charter-schedule`, and both wardens inherit it.

### 3.4 Across devices

Simultaneous use of two devices must not double-spend: chatting on the phone
while YouTube plays on the laptop is one hour, not two.

**This is already built and has never run.** `UsageSyncPayload`
(`core/crates/charter-proto/src/usage_sync.rs`, kind 31115) carries each
device's active minutes as a `MinuteSet` bitmap, and `pooled_used`
(`core/crates/charter-schedule/src/budget.rs:42`) computes the pooled day as
`|own ∪ elsewhere|` with the comment *"simultaneous use across devices counts
once"*. It is implemented, tested, and wired into the budget path on both
platforms.

It is gated on `Child.dependantPubkey`, which MyCharter sets to `null`
(`apps/charter-app/src/store/store.tsx:677`) and never mints, so
`store.tsx:1101` skips every send. A two-hour cap silently grants four hours
across a laptop and a phone today.

Switching it on is not invention. It is also **coupled to Linux web
filtering** — the same subject-identity plumbing carries both, and enabling one
without the other breaks filtering silently. They ship together or not at all.

## 4. Wire changes

Deliberately small. Three additions, no removals, every existing charter keeps
its exact current meaning.

### 4.1 `budget.model`

`GrantBudget` gains an optional `model: "session" | "named"`. **Absent means
`session`**, so every clause already signed keeps byte-identical semantics and
no existing ward changes behaviour on upgrade.

`hours` is not a value: a charter with a schedule and no budget clause already
*is* the hours tier. The guardian app names it; the wire does not need to.

### 4.2 Buckets carry the cost set

`AppBucketRule` already is `{id, label, apps[], dailyMinutes?, weeklyMinutes?}`.
Its reading becomes model-dependent:

- **`session` model** — unchanged. A bucket is an extra allowance on top of a
  day that is charged anyway.
- **`named` model** — bucket membership *is* the definition of "this costs".
  A bucket with no allowance means "costs the shared day budget, nothing more".

This needs one validation relaxation: today at least one of
`dailyMinutes`/`weeklyMinutes` is required. In `named` mode a bucket with
neither is legal and meaningful. In `session` mode it stays required, because
there it would mean nothing at all.

No new clause, no new store key, and the app picker, per-device rules, holds and
ask-first all keep working against the same vocabulary.

### 4.3 A `site:` identity form

Site apps can currently only be free, because the `learning` clause both
*defines* a pinned window and *makes it free*. Those two facts must come apart.

**`learning` is retained as the site-app catalogue.** It already carries exactly
a site-app definition — `{id, label, kind, url, domains, exec, trusted}` — and
the enactor already materialises launchers from it. In `session` model it also
means "free", exactly as today. In `named` model it means only "these windows
exist and are pinned"; whether one costs is decided by the buckets clause like
anything else.

To let a bucket name one, identities gain **`site:<id>`**, following the
precedent `cmdline:` set. It matches a process whose command line is a
sanctioned launch of that site app — the same `site_app::is_sanctioned`
predicate the kill sweep uses, so what is metered is exactly what is stopped.

`isDeviceShaped` in the guardian app is extended to accept it, which is what
stops `partitionOnPolicyChange` silently dropping a site when its group changes
policy.

### 4.4 The minute journal must follow the model

A trap found while mapping the core, worth stating because it is silent if
missed. `UsageLedger::credit_bucket` (`core/crates/charter-schedule/src/usage.rs:195-218`)
marks the minute journal for **both** `Bucket::Screen` and `Bucket::Learning` —
today both are "time at the glass", which is right when the baseline charges
anyway.

In `named` mode that would be wrong. The journal is what the cross-device union
pools (§3.4), so marking a minute in which nothing cost would let free time on
the laptop consume the phone's allowance — an over-charge arriving from a
completely different device, which is exactly the unexplainable failure this
whole design is meant to remove.

**Rule: in `named` mode the journal is marked only for minutes that cost.** The
scalar meters and the journal must agree about what a spent minute is, or
pooling silently contradicts the local clock.

## 5. What this deletes

This is a subtraction. Net of the additions above:

- The free-time **grant** disappears from the `named` tier entirely, and with it
  the class of attack the ownership/argv/symlink hardening exists to stop. That
  hardening stays (it still guards the *sandbox*), but nothing depends on it for
  the correctness of the clock.
- `focus::classify`'s Learning arm, the learning cap, the free-versus-counted
  fail-direction asymmetry (learning fails closed, buckets fail open) and the
  "learning suppresses unrecognised" interlock all become `session`-only legacy,
  and the guardian app stops offering them above that tier.
- The named-times editor collapses from three policies to one question per app:
  **does this cost, and does it have its own allowance.**
- No window geometry, monitor enumeration, stacking order or occlusion code is
  ever written.

## 6. Failure directions, chosen deliberately

**Coverage, not accuracy, becomes the parent's job.** A newly installed game is
free until it is named. The characteristic failure of the system flips from
*charging a child for something it shouldn't* to *failing to charge for
something it should*.

That is the right error for this product. A false charge is unrecoverable — it
teaches a child the system is arbitrary and cannot be reasoned with. An
undercount is a gap you close when you notice.

**The unrecognised-time counter becomes load-bearing.** Built as an honest
curiosity (§2.3 of the honest-attribution design), it reports foreground time
whose identity matched nothing the guardian named. In `named` mode that is
precisely the signal "something ran and nothing says whether it costs", and it
is what keeps the cost list current. Its existing under-reporting caveats stand
and must keep being stated plainly.

**Invisible charging is answered with transparency, not with a weaker rule.**
The tray shows, live, what is costing right now. If he can see it, he can close
it; nothing happens behind his back. Transparency is the invariant.

## 7. Known residual holes, stated up front

- **A game that minimises to tray escapes.** Rare — games generally do not — but
  real. Named rather than papered over.
- **`_NET_WM_PID` is declared by the application, not guaranteed by the
  kernel.** A determined ward could write a program that lies about which
  process owns its window. This is not new: the current focused-window path
  places the identical trust. In the new model it is the *evasion* direction,
  which is the safer one to be soft in, and the unrecognised counter is the
  backstop.
- **A browser is one window with many tabs.** Firefox either costs or it does
  not. Site apps are what make this precise — which is why §4.3 is not optional
  polish but the thing that makes the tier usable at all.
- **Offline pooling is generous.** A device enforcing on a stale consolidated
  view under-counts the pool rather than over-counting it, so a flaky relay can
  never cause a wrongful early lock. Deliberate, and unchanged from the existing
  design.

## 8. Build order

Each stage is independently shippable and independently testable.

1. **`site:` identities + sites carry a policy.** Everything downstream is
   imprecise without it. Core identity form, `site_app` match, guardian-app
   picker, golden vectors both sides.
2. **`budget.model` + the window-set probe + counting once.** The attribution
   change on Linux, the model switch in the shared enforcer, Android's
   foreground reading of the same model.
3. **Tiers in the guardian app.** The exclusive-mode UI, the collapsed
   named-times editor, the copy that says what a tier gives up.
4. **Pooled usage switched on, with the web-filtering fix beside it.** Never
   apart.
5. **Tray transparency** — what is costing right now.

## 9. Hardware gates (decented only)

Nothing here is verified until it runs on Rob's machine.

- **Dual screen:** full-screen game on one output, Khan in front on the other.
  Expect: the day counts down. Close the game: it stops.
- **Tray app:** WhatsApp/Discord open, then closed to tray. Expect: costs while
  its window is up, stops when it folds to the tray, and the tray list agrees.
- **Minimise:** YouTube minimised with audio playing. Expect: still counting.
- **Phone + laptop together:** both active for a measured ten minutes. Expect
  ten minutes spent, not twenty.
- **A week of living with it**, which is the only test that finds the copy
  problems.
