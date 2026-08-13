# Approvals Architecture — Charter is the Face, Signet is the Vault

**Date:** 2026-07-22
**Status:** Decided (design). No implementation started under this doc.
**Participants:** decented (decision), Claude (design)

## Context

Approvals today live in MyCharter (the guardian console PWA): the local guardian
signer, the Approvals screen, and the relay intake (`subscribeRequests.ts`)
that unwraps gift-wrapped device REQUESTs. Two pressures forced a decision:

1. **The wake problem.** MyCharter is a PWA with no push path at all — the
   relay subscription only runs while the app is foregrounded. A locked phone
   never learns that a ward asked for more time. A native carrier is required
   in every version of the future.
2. **The Signet question.** `spec/contract.md` still describes Signet as "the
   guardian app" that approves, and Signet is the natural home for signing —
   especially on a long time frame where wards sign into many Nostr apps
   (games such as Axe & Stacks, and eventually more). Meanwhile MyCharter's
   local signer had begun accreting vault features (encrypted key
   backup/restore). Left undecided, drift would build a second vault by
   accident.

The resolution rests on splitting "where approvals live" into four layers with
different reversibility:

| Layer | What it is | Reversibility |
|---|---|---|
| **Authority** | Whose key signs guardian decisions | Semi-sticky (custody migration is delicate) |
| **Stream** | The asks: gift-wraps addressed to the guardian pubkey | Permanent, and deliberately brand-neutral |
| **Surface** | Which app renders the inbox | Fully reversible (any authorised client can subscribe) |
| **Wake** | Which native app lights the guardian's screen | Singular per phone, but assignable and migratable |

The stream being brand-neutral (the contract explicitly allows other bunkers)
means the long-lived commitment is already safe. What follows are the
product-level assignments on top of it.

## Decisions

### D1 — Charter is the sole human-facing approval surface

Every guardian-facing ask — device time extensions, app requests, APK
installs, ward sign-ins to new Nostr apps, in-app asks from Charter-aware
games — lands in **one inbox, in Charter**. The notification says Charter, the
approve/deny sheet is Charter, the policy console is Charter. Signet has no
wardship UI.

Asks that *originate* in Signet's domain (e.g. the bunker's auto-policy cannot
decide a sign-in) are emitted onto the wire like any other REQUEST and land in
the same inbox. Signet enforces; Charter adjudicates.

### D2 — Signet is the vault

Signet's role is custody and signing: it holds keys (ultimately in secure
hardware) and signs the guardian's decisions. There is exactly **one** native
key-holder; it is Signet. Charter surfaces rent authority from it.

**Discipline rule, effective now:** anything vault-shaped (backup, rotation,
multi-guardian, key ceremonies) belongs to the Signet component. MyCharter's
local signer receives only what shipping demands, and is understood as a
bootstrap whose custody eventually hands up to the vault. The custody hand-up
(local key → Signet vault) gets a migration design *early* — specified before
it is needed, because authority is the semi-sticky layer.

### D3 — The vault never prompts

For wardship decisions, Signet either **auto-signs its paired Charter
console's ruling** (a trust established at pairing) or refuses. It never pops
its own "approve?" dialog for kid-related asks, so the guardian is only ever
asked once, by one brand. The moment the vault grows a second voice, this
architecture has failed. (Signet's own adult-facing UX — pairing its owner's
apps — is unaffected.)

### D4 — Signet is embeddable: one custody codebase, two packagings

- **Charter-only families:** Signet ships *inside* the Charter app as the
  custody engine. One icon; the guardian never encounters the word Signet or
  anything Nostr-shaped.
- **Nostr-native families:** standalone Signet holds the keys; Charter binds
  to it (NIP-46 / local IPC) under the D3 auto-sign trust.

This dissolves the "two permanent custody tiers" tax: it is one vault with two
skins, not two custody systems.

### D5 — One wake channel: a thin native Charter carrier

A thin Charter APK carries notification duty on the guardian's phone: a
foreground service holds the relay connection and raises a full-screen /
heads-up approve prompt, wrapping the existing MyCharter web UI. Design notes:

- **No Google dependency:** the foreground-service-holds-the-socket design
  needs no FCM, works on de-Googled guardian phones, and rides our own relay.
- Wake duty is assignable: if standalone Signet later becomes the resident
  native app for a family, it can carry the wake and deep-link into Charter UI.
  Nothing on the wire changes.
- Known hazard: Android 14+ restricts `USE_FULL_SCREEN_INTENT` — may need a
  user grant or heads-up fallback.
- Web Push in the PWA remains a possible *secondary* nudge (e.g. desktop
  guardians), never the primary channel.

### D6 — In-app asks: three enforcement layers, one adjudication point

| Layer | Example | Enforced by |
|---|---|---|
| OS | Device time, which apps exist, hotspot | warden / Device Owner |
| App-slot | Which apps during which windows | warden / Device Owner |
| In-app | Session length, feature gates, mods inside a game | **The app itself** (Charter-aware) |

Enforcement is distributed to whoever owns the territory; deciding stays in
the one inbox. A Charter-aware app reads its clauses and enforces them
silently; when it hits an ask its clauses cannot decide ("install this mod?"),
it emits a wrapped REQUEST to the guardian pubkey and honours the signed GRANT
that comes back. It never grows a second voice to the guardian (no in-game
parent PIN, no email-a-code).

**Axe & Stacks is designed from day one as the published template** for
Charter-aware apps — the reference implementation of the consumer-app side of
the contract, not a private first-party hookup. The contract's wire-level
brand-neutrality is what makes this a category any third-party app can adopt.

### D7 — Every ask offers "make this a rule"

Approvals are also the policy-authoring surface. Each ask is answered as
*approve once* or *approve and create the standing clause* — converting live
decisions into silent auto-policy. This is the mechanism that keeps ping
volume sane as ward activity grows (asks are ongoing, not set-up-once), and it
is why the inbox stays in one place: the surface that adjudicates is the
surface that authors policy.

### D8 — The ward always knows where they stand

The ward gets a **read-only mirror** of their own charter, on their own device:

- **Time remaining**, at a glance — on Android, a home-screen **widget** plus
  the ward-side Charter app; on Linux, the existing console surface.
- **Their schedule**: when they're allowed, how much per day — the same facts
  the guardian sees, mirrored. View, never edit.

No surprises: enforcement the ward can't predict reads as arbitrary power, not
a charter. This is the ward-side twin of "enforce, don't surveil" — *govern,
don't gaslight*. On Android this lives in the existing ward-device Charter app
(the Device Owner app grows a status screen + widget); it is **not** part of
the guardian carrier APK.

### D9 — The communication lifeline

**Enforcement must never leave a ward unable to call for help.** A locked
phone is still a phone:

- The **dialer and emergency calling are exempt from every clause** — never
  suspended, never blocked by lock state, hotspot filtering, or any future
  rule. Non-negotiable floor, on by default, not configurable off.
- **Guardian numbers are always callable** (and ideally a small
  guardian-approved lifeline list — e.g. grandparents), even mid-lockout.

Detailed design (allowlist model, how lock/kiosk surfaces the dialer, whether
messaging apps join the lifeline) is a follow-up — but the principle is
decided and binds every enforcement feature, existing and future.

## Approval flow (canonical walk)

1. Ward taps "ask for more time" (or a Charter-aware app hits an undecidable
   gate) → a gift-wrapped REQUEST addressed to the guardian pubkey goes to the
   relay. Brand-neutral wire, unchanged from today.
2. The Charter carrier (D5) wakes the guardian's phone; the Charter sheet
   shows the ask.
3. Guardian taps Approve (optionally "…and make it a rule").
4. Charter asks the vault to sign the GRANT; Signet signs **silently** (D3) —
   an internal call in the embedded packaging, a paired auto-sign in the
   standalone packaging.
5. The GRANT flies back; the warden / Device Owner / Charter-aware app
   enforces it.

One human touch, one brand, in both packagings.

## What this does NOT decide

- Implementation sequencing and scheduling (separate planning).
- Carrier APK internals (service lifetime, battery posture, full-screen-intent
  strategy).
- Wire details for consumer-app asks: kind numbers, transport (via the ward's
  Signet NIP-46 channel vs direct relay publish), grant caching / offline
  behaviour inside games. These go to `spec/contract.md` as a consumer-app
  section with **[decide]** tags like the existing brokering sections.
- The custody hand-up mechanics (to be designed early, per D2, but not here).

## Consequences / follow-ups

1. **`spec/contract.md` reconciliation:** the contract still names Signet as
   "the guardian app" that approves (e.g. the roles table) — predating
   MyCharter's approvals. Update to: Charter console = guardian surface;
   Signet = bunker/vault. Fold in the wardship lexicon rollout that is already
   pending.
2. **Charter carrier APK** is the next buildable increment — needed in every
   version of the future; wake duty migrates freely later.
3. **Custody hand-up design** (local key → Signet vault) — spec early.
4. **Consumer-app contract section** — spec the ask/grant/clause surface for
   Charter-aware apps; Axe & Stacks implements it as the template.
5. **Signet component boundary** — define what "embeddable Signet" exports
   (custody + signing + policy engine) so Charter can consume it in-process.
6. **Ward-side mirror (D8)** — status screen + time-remaining widget in the
   ward-device Charter app; schedule view. Read-only.
7. **Lifeline audit (D9)** — verify today's Android enforcement (suspend
   lists, lock, hotspot) can never block the dialer/emergency calls; design
   the guardian-numbers allowlist; then bind the rule into every future
   clause's review checklist.
