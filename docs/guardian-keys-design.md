# Guardian keys: a family root and working keys

Status: **proposed**, for review. Nothing here is built. Once agreed, the wire
parts move into `spec/contract.md` and this note is cut down to the reasoning.

## Why

Today a family has one guardian key. It is generated and kept in the guardian
app on one phone, and every ward pins it at pairing. That gives us four
problems:

- **A lost phone means setting every device up again.** The key backup exists
  because of this, and most families will never make one.
- **A second parent can only be the same key copied,** which cannot be taken
  away from one parent without the other.
- **The best custody we can offer, a key generated on a Heartwood through My
  Signet, can't carry the load.** A Heartwood must be powered and on Wi-Fi,
  answers over a relay, and decrypts at roughly one item a second. Every
  ward's STATUS, usage and request is encrypted to the guardian key, so a
  hardware-held guardian key would be decrypting all day, and nothing would
  work while it was switched off.
- **Kintrinsic is the only Kindred app with its own guardian identity.**
  Kindependence requires My Signet and Kinterest is built around it.

The fix is the usual one for hardware-held keys: the strong key signs rarely,
and day-to-day work is done by keys it has vouched for.

## The model

- **Family root key (R).** The family's guardian identity: the key wards pin.
  - Best: generated on a Heartwood through My Signet, and never leaves it.
  - Also supported: held in My Signet as a software key, or held in the
    guardian app on the phone (today's set-up, and the zero-friction start).
- **Working key (W).** One per guardian install: each parent's phone and each
  guardian browser. Generated on that install, encrypted at rest as the
  guardian key is today, and never copied anywhere.
  - Signs clauses, seals, GRANTs and pair offers.
  - Receives ward traffic.
- **Approval (A).** R signs one event per working key: "W speaks for me as a
  guardian, labelled 'Sam's phone'". Withdrawing it is a separate, permanent
  statement from R for that W.

R signs only approvals, withdrawals and a root move. That is a handful of
signatures in a family's lifetime, so a Heartwood button press for each is
fine. It is even a feature: adding a parent needs a physical press.

## What changes for wards

- A ward still pins **R** at pairing, and the pairing link is unchanged.
- A ward accepts an approval only if R signed it. It keeps the newest approval
  per working key. The pairing floor (`issuedAt >= paired_at`) does **not**
  apply to approvals: a working key approved before this ward paired still
  counts.
- A clause, GRANT or pair offer is accepted if it is signed by R itself, or by
  a W whose newest approval is active. Everything else about clause checking
  stays as it is.
- **A withdrawn working key can sign nothing new.** The ward refuses any
  clause, GRANT or pair offer from it whose arrival follows the withdrawal,
  whatever its `issuedAt`: a stolen phone can backdate `issuedAt`, so time
  cannot be trusted here.
- **Clauses it already signed stay enforced until a live key replaces them,
  kind by kind.** Dropping them at once would fail open: an absent schedule is
  "unconstrained", and relays do not guarantee that a ward sees the re-signed
  set before the withdrawal. So the guardian app re-signs the family's current
  clause set with a live working key and sends it **before** the withdrawal.
  The pairing floor and replay rules apply to the re-signed set as usual.
- Stored clauses keep their signer's pubkey. This is a store-format change on
  both wards, and it is what lets the screen say who set a rule.
- Wards encrypt STATUS, usage and requests to **every active working key**
  they hold an approval for, not to R. R is never asked to decrypt anything.
  A working key whose approval a ward hasn't received yet gets nothing from
  that ward until it has. Requests fan out once per active key.

**How approvals reach a ward.** A ward requires a seal's author to be the
inner event's author (`charter-proto` NIP-59 unwrap). An approval is signed by
R, but R must not be asked to seal one per ward. So there is one narrow
exception, for approval statements only:
- the seal may be authored by the very working key the approval names, and by
  no other key;
- R's signature proves R approved W, and the seal proves the sender holds W.

A working key sends its own approval to each ward, gift-wrapped over relays and
the home network, before or alongside its first clause. A scan-to-pair offer
(31117) signed by W carries W's approval in the same delivery, so a new ward
can check it at once. Approvals are never published openly: the label and the
family's shape stay private.

## Two parents

- **Equal authority.** Either parent's working key can set any clause. For one
  clause kind, the newer `issuedAt` wins, exactly as with one guardian today.
  This matches Signet's joint-custody draft ("either parent approves").
- **Requests:** a ward's request goes to every active working key. The first
  GRANT the ward accepts settles it. The other parent sees it answered from
  the ward's next STATUS.
- **Who did what:** every clause carries its signer. The guardian app and the
  ward's own screen show "set by Sam's phone" using the approval's label.
- **Removing a parent:** the other parent's app re-signs the current clauses,
  then R withdraws that parent's working key. This needs R, so with a
  Heartwood it needs the Heartwood.

## Journeys

**Heartwood first (the ideal).**
1. The parent sets up My Signet and a Heartwood, and creates the family
   identity on the Heartwood.
2. In Kintrinsic, they choose "Connect My Signet". Kintrinsic connects to that
   identity's slot over NIP-46 and generates its working key.
3. Kintrinsic asks R to sign the approval. The parent presses the Heartwood
   button.
4. Kintrinsic shows pairing codes that carry R. Wards pair as today.

**Phone first (today's families, and the quick start).**
- R stays on the phone, in the guardian app. Kintrinsic can sign with R
  directly, so nothing changes for the family until they want it to.
- Adding a second parent, or moving the root somewhere safer, is offered from
  Family.

**Moving the root.** A key can't be moved onto a Heartwood: only seeds can be
restored to one, and that passes through the host. So moving the root means a
new root R2 and a **root move** event. The old R signs "the family root is now
R2", and every ward that pinned R switches its pin to R2. The old R is deleted
from the phone only once every ward has confirmed the move in STATUS.

Anyone holding R can already set any rule, but a root move would make that
**permanent**, locking the real parent out for good. So a ward does not take
a root move silently. It shows the same six-digit check it shows at pairing,
for R2, and a grown-up confirms it on the device. This happens once per ward,
and rarely. See decision 5.
- If the old root is already lost, there is nothing to sign the move. Those
  wards have to be paired again.
- That is why the guardian app should offer the move early, as part of
  backing up.

**Lost phone** (with the root elsewhere): from the other parent's app, or from
My Signet, withdraw the lost phone's working key. Then set the phone up again
with a new working key. No ward needs touching.

## Wire sketch: Kindependence's device statements, reused

Kindependence (another Kindred app) already ships this pattern, with tests:
- a local phone key per install;
- a **device statement** signed once by the My Signet identity;
- a permanent revocation.

Its statement is a kind-30078 event (NIP-78 app data), and My Signet already
signs it for Kindependence. Kintrinsic reuses the same shape with its own `d`
prefix and scope. That makes "a family identity vouching for a device" read
the same across the suite, while a phone trusted for Kindependence's location
circles is not thereby a Kintrinsic guardian.

| Event | Kind | `d` | Signed by | Tags and content |
|---|---|---|---|---|
| approval | 30078 | `kintrinsic/device/<W hex>` | R | `p` = W, `scope` = `kintrinsic:guardian`; content `{v:1, issuedAt, label}` |
| withdrawal | 30078 | `kintrinsic/revoke/<W hex>` | R | `p` = W; content `{v:1, issuedAt}` |
| root move | 30078 | `kintrinsic/root/<new R hex>` | old R | `p` = new R; content `{v:1, issuedAt}` |

Rules, taken from Kindependence where it already solved them:
- **Proof of possession:** the seal that carries an approval must be authored
  by the W in its `p` tag (the seal exception above).
- **One identity per working key:** a ward binds a W to the first R that
  approved it, and ignores approvals for that W from any other key.
- **Withdrawal is permanent.** It has its own `d`, so no later-dated approval
  can bring a withdrawn W back. A family that wants the phone back gives it a
  new working key.
- **Order doesn't matter:** a withdrawal that arrives before its approval is
  kept, not dropped, and wins when the approval arrives. In a same-second tie,
  the withdrawal wins.
- **Fail-safe:** a malformed statement or an unknown `v` is ignored, and the
  working key stays unapproved.
- **My Signet and the Heartwood:**
  - Kintrinsic's slot needs `sign_event` for kind 30078 only, which isn't on
    Signet's never-listed list (24133, 31000).
  - The slot can be set to ask every time.
  - One check is needed: Signet may scope 30078 by `d` prefix, and if so,
    `kintrinsic/` has to be allowed.
- Old wards understand neither kind. The guardian app keeps signing with R
  directly for any child that has a ward below the version that supports this
  model, and says so ("update Robin's phone to add a second parent").

## Decisions needed

1. **One family root, or one per parent?**
   - **(a) One family root.** Both parents' working keys sit under one R. That
     R lives in one parent's My Signet or Heartwood, so that parent alone can
     add or remove the other. This is simple for wards (one pin).
   - **(b) Each parent's own identity.** This is how Kindependence does it:
     each parent's own My Signet identity is a guardian root. A ward pins
     both: the first at pairing, the second through a co-guardian statement
     from the first, confirmed on the device like a root move. Each parent's
     working keys sit under their own root. Neither parent's custody depends
     on the other's phone or Heartwood, and it matches "keys once across
     Kindred".
   - Either way, both have equal authority over rules, and the newest wins.
   - **Decided 2026-10-28: (b).** It costs one more statement and a pin list
     on wards, and it is the version that survives two households.
2. Is **R the parent's main My Signet identity,** shared with Kindependence
   and Kinterest (keys once across the suite), or a derived "Kintrinsic
   guardian" persona? A shared identity is the Kindred goal, but it means
   Kintrinsic's slot can sign approvals with the parent's main identity.
3. **Reusing Kindependence's kind-30078 statement** (recommended) or new
   Kintrinsic kinds. Reuse means My Signet already knows how to sign it.
4. Does the **child's identity** (a dependant key shared across Kindred apps)
   come into scope now, or later? This design leaves the ward's own identity
   as it is.
5. **Root move confirmation on each ward** (recommended), or signature-only?
   Confirmation costs one tap per ward and closes the permanent lock-out.
   Signature-only is smoother but makes a stolen root unrecoverable.
6. **How the second parent's app learns the family.** A newly approved working
   key knows no children, devices or names: those live only in the first
   parent's app. Recommended: an encrypted **family roster**, a new kind
   signed by a working key and gift-wrapped to the other active working keys.
   It carries children, names, colours and device pubkeys, and the wards'
   STATUS fills in the live state. The alternative, wards announcing
   themselves to a new key, can't supply names. Without one of these, "add a
   second parent" opens on an empty Family screen.

## Build order (once agreed)

1. Contract and vectors:
   - the three statements (plus the roster kind, if decision 6 goes that way);
   - ward acceptance, the seal exception for approvals, and the withdrawal
     rules;
   - root move and its confirmation.
2. Wards (core, Linux, Android):
   - the approval store, the signer check, and the stored-clause format
     carrying the signer;
   - multi-recipient STATUS, and root move with confirmation.
   - Old wards keep working.
3. Guardian app:
   - working keys, and signing with W;
   - "Connect My Signet" as R;
   - add and remove a parent;
   - move the root;
   - Signet path parity (GRANTs, Approvals inbox, reconnect after reload);
   - Heartwood listed as "through My Signet", replacing the mock.
4. Hardware gates:
   - a Heartwood-first family from scratch;
   - adding and removing a second parent;
   - moving a phone root to a Heartwood with wards already paired;
   - a lost phone.
