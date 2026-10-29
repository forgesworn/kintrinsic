# Guardian keys: open questions for a design session

These questions came up while building *Guardian roots and working keys (v1)*
(`spec/contract.md`). The build does not decide them. Each entry records the
reading the build took in the meantime, which is always the fail-closed one.

Format: one `##` heading per question, then **Where** (contract clause, code
path), **Question**, **Build's interim reading**, **Raised by** (batch).

## Same d, same issuedAt, different content

**Where:** rule 2; `src/guardians/state.ts` `orderAgainst`.

**Question:** two statements share a `d` and an `issuedAt` but differ (another
label, another `expiresAt`, another signer). Which one wins?

**Build's interim reading:** the statement with the lower event id (as
lowercase hex) wins the tie, whichever arrived first, so every ward converges
on the same one; the loser is dropped as `tie_not_newer`. Re-delivery of the
held event itself is `duplicate`. (Batch 1 kept the first received; the
review showed that two wards receiving a pair in different orders then held
different pending signers.)

**Raised by:** batch 1.

## Same-second ties across different d

**Where:** rule 2 ("A withdrawal, removal or cancel wins a same-second tie");
`src/guardians/state.ts`.

**Question:** a withdrawal, removal or cancel never shares a `d` with the
statement it acts on, so a tie "among statements with the same `d`" can't
involve them. What does the tie rule bind?

**Build's interim reading:** it binds across the related `d`s. A withdrawal of
W beats an approval of W at the same `issuedAt` (rule 3 makes that
unconditional anyway). Re-pinning a removed root needs a co-guardian strictly
newer than the removal. A cancel drops a pending statement whose `issuedAt`
equals its own.

**Raised by:** batch 1.

## 300-second future refusal for GRANT, RELAYS and USAGE_SYNC

**Where:** *Who may sign what* ("a CLAUSE or statement whose `issuedAt` is
more than 300 seconds ahead … is refused"); `acceptSignedEvent`, `applyRelays`.

**Question:** does the 300-second refusal also apply to GRANT, RELAYS and
USAGE_SYNC, which carry `ts` or `created_at` rather than `issuedAt`?

**Build's interim reading:** yes for GRANT and USAGE_SYNC: the reference
refuses either if its `ts` is more than 300 seconds ahead, on top of their
existing skew rules. RELAYS keeps its existing, stricter rule (no future
`created_at` at all, at most 600 s old, not before pairing, newer than the
last applied), which the reference models.

**Raised by:** batch 1.

## Cancel or objection before its statement

**Where:** *Pending statements* (cancel), rule 7 (objection);
`cancelledBy`, `objectedBy`, `processCancel`, `processObjection`.

**Question:** a cancel or objection can arrive before the statement it
targets (relays don't order delivery). Is it dropped, or held? If held, does
an objection kill only removals at or below its `issuedAt`, or any later
removal too?

**Build's interim reading:**
- Both are held. When the target arrives it is dropped (`cancelled` /
  `objected`) if the held cancel was signed by the target's signer, or the
  held objection was signed by the root being removed and names the removal's
  signer as `p`, and the target's `issuedAt` is at or below the held one's.
  For an objection, see also "Removal re-issued after an objection".
- An objection that meets an already pending removal by its `p` drops it
  whatever the two `issuedAt` values: the contract puts no time condition on
  an objection (batch 1 wrongly applied "at or below" here too).
- Cancels are stored per (`d`, signer) and objections per (`d`, `p`), so one
  root's cancel never shadows another's, and an objection against one remover
  never displaces one against another.
- An objection naming a remover other than the pending removal's signer is
  held, not refused.

**Raised by:** batch 1.

## Held-statement quota

**Where:** rule 10 ("256 tombstones and 64 held early withdrawals … A root at
its quota can't approve new working keys"); `rootAtQuota`, `heldCount`.

**Question:** do held cancels and objections count toward the 64? Does
reaching the held quota (not only the tombstone quota) stop the root
approving new working keys?

**Build's interim reading:** held early withdrawals, held cancels and held
objections all count toward one quota of 64 per signing root; past it, more
are dropped (`held_quota`). A root at either quota (256 tombstones or 64
held) can't approve new working keys (`root_at_quota`). Withdrawals and
removals still apply at quota.

**Raised by:** batch 1.

## Working keys after a root move or removal

**Where:** rules 1, 4, 7 and 8; `processApproval`, `processWithdrawal`,
`applyPresenceConfirm`.

**Question:** an approval by R2 for a working key bound to R is dropped
before the move (R2 isn't pinned), and after the move that key has departed.
How does a W "travel with the move" other than the confirming one? And what
of R's pending statements, a removed root's keys after it is re-pinned, and
a withdrawal of a key that departed with a move?

**Build's interim reading:**
- At confirmation, only the confirming W (with R2's inline approval) moves to
  R2. R's other working keys depart (fail closed), but R2, and only R2, may
  re-bind one later with a newer approval (`approval_rebound`); this is the
  "one re-binding" of rule 1.
- R's pending and held statements are dropped when the move applies, as R2's
  are on removal. Pending statements naming the departed root are dropped too.
- A removed root's working keys stay departed for good, even if the root is
  re-pinned later; it approves new ones.
- A withdrawal of a departed key by its old root (once re-pinned) or by the
  root that may re-bind it is applied, making the departure permanent.
- A moved-away root is re-pinned only by a co-guardian newer than the move,
  as for a removed root.
- A removed or moved-away root can't come back as another root's working
  key (`key_departed`), in an approval or a presence confirmation; it returns
  only as a root, by a co-guardian confirmed afresh (batch 1 review).

**Raised by:** batch 1.

## expiresAt on non-pending statements

**Where:** statement content (`expiresAt?`), *Pending statements*;
`processApproval`, `pairFromOffer`.

**Question:** the content allows `expiresAt` on every statement, but only
pending ones say what it means. What does it mean on an approval, withdrawal,
cancel or objection?

**Build's interim reading:** an approval carrying `expiresAt` is refused
(`expires_at_not_allowed`), in delivery, in a presence confirmation and in a
PAIR_OFFER, rather than guess at an expiring key. On a withdrawal, cancel or
objection it is ignored (refusing those would fail open). The guardian-side
builders never set it on these.

**Raised by:** batch 1.

## Sealing an objection

**Where:** *Delivery and the two seal exceptions*, exception 2 (lists
withdrawal, co-guardian, root removal, root move and cancel); `sealAllowed`.

**Question:** the objection is missing from exception 2. Is that deliberate,
so a Heartwood-held root must seal its own objection?

**Build's interim reading:** yes, as written: an objection must be sealed by
its signer, the root being removed. One sealed by that root's working key is
dropped (`seal_mismatch`).

**Review argument (batch 1 review), for the design session:** this makes the
defence harder than the attack. R1's removal of R2 can be sealed by R1's phone
under exception 2, but to object, R2's root (often on a Heartwood or in My
Signet) must both sign and seal to every ward. If that root is out of reach
for the 72 hours, the removal applies against R2's will. The reviewer reads
the objection's absence from exception 2 as an omission and recommends adding
it (a W bound to the objecting root and in the signer set may seal it). The
build keeps the literal reading until the design session decides; the vector
`rule7_objection` pins it and would flip.

**Raised by:** batch 1.

## Removal that expires before it can take effect

**Where:** rule 7 (takes effect at the later of confirmation and `issuedAt` +
72 h), *Pending statements* (expiry); `advance`.

**Question:** a removal whose `expiresAt` falls before `issuedAt` + 72 h can
never take effect. And if the two times are equal, which wins?

**Build's interim reading:** confirmation does not stop the expiry clock. A
removal takes effect only if `takesEffectAt` is strictly before `expiresAt`;
otherwise it expires at `expiresAt`. Timers are processed in time order,
ties by `d`.

**Raised by:** batch 1.

## Presence confirmation details

**Where:** *Presence confirmation*; `registerPresenceToken`,
`applyPresenceConfirm`.

**Question:** the contract doesn't say whether a token is minted for one
pending statement, which tags kind 31121 carries, when a token is spent, or
whether a root can confirm as itself.

**Build's interim reading:**
- A token may be bound to one pending statement's `d` when minted (or left
  open with `null`); a bound token confirms only that statement.
- Kind 31121 carries the `["t","charter-device"]` marker, as other inner
  events do, and is refused without it.
- A token is spent only by a successful confirmation, as at pairing.
- The confirming key must be a working key: the inline approval for a
  co-guardian or move can't name R2 itself, and a removal confirmation must
  come from an active W bound to the removing root.
- A second confirmation of an already confirmed removal is dropped
  (`duplicate`).
- A confirmation re-checks R2 as it stands then: already pinned
  (`already_pinned`), now a working key (`key_role_conflict`), departed since
  (`not_newer_than_departure`), and R2's tombstone or held quota
  (`root_at_quota`). When a co-guardian or move pins R2, every other pending
  statement naming R2 is dropped, so R2 can't be pinned twice (batch 1
  review).

**Raised by:** batch 1.

## Cancel targets

**Where:** statement table (cancel `d` = `kintrinsic/cancel/<d of the pending
statement>`); `parseStatement`.

**Question:** what if a cancel's `d` names something that is never pending,
such as an approval?

**Build's interim reading:** the cancel is malformed
(`bad_cancel_target`). Likewise an objection must name a removal's `d`
(`bad_objection_target`).

**Raised by:** batch 1.

## Where the roster carries its approval

**Where:** *Family roster* ("carries the sender's approval"); `familyRosterTemplate`,
`mergeRoster`.

**Question:** the roster content is given as `{ v: 1, children }`; where does
the sender's approval go, and what tags does kind 31119 carry?

**Build's interim reading:** the approval travels in the content as
`approval` (as in PAIR_OFFER and the presence confirmation). The builder adds
the marker tag; the merge does not require any tag. A same-second record does
not replace the local one.

Batch 1 review: "approved by a root that the family's wards report" is read
as "listed under that root in the wards' STATUS `guardians.roots[].keys`", so
a withdrawn W's rosters are refused (`key_not_reported`). A child record more
than 300 s ahead of the receiving install's clock refuses the whole roster
(`future`), so a far-future `issuedAt` can't pin a child entry for ever.

**Raised by:** batch 1.

## Guardian PIN salt and encoding

**Where:** *Presence confirmation*, the Android guardian PIN;
`src/guardians/pin.ts`, `vectors/guardians/pin.json`.

**Question:** the salt's length and the PIN's byte encoding aren't given, nor
whether "after 5 failures" counts the 5th failure itself.

**Build's interim reading:** the salt is lowercase hex of 16 to 64 bytes; the
scrypt input is the PIN's UTF-8 bytes and the scrypt salt is the decoded salt
bytes. After the 5th consecutive failure the wait is 60 s, then 120, 240 …
capped at 3600 s; fewer than 5 failures wait nothing.

**Raised by:** batch 1.

## Android pairing has no PAIR_OFFER

**Where:** *Scan-to-pair with a working key*; `android/jni/src/warden.rs`
`Warden::pair`, `ui/PairingSas.kt`.

**Question:** the Android ward pairs by opening a `bunker://` link (guardian
key + `&token=`, checked by a six-digit SAS), not by a PAIR_OFFER. The contract
only says how a PAIR_OFFER sealed by a working key pins its root. Which key
does the Android link name when the guardian install has a working key?

**Build's interim reading:** the link names the root R, and the SAS covers R
and the token, as today. The ward pins R. The working key then sends its own
approval (sealed by W) before its first signed event, as for any W. Nothing
in the link is trusted beyond what the SAS already covers.

**Raised by:** batch 2 (planning).

## How a guardian PIN reaches an Android ward

**Where:** *Presence confirmation*, the Android guardian PIN (`guardianPin`
inside the RELAYS body); rule 11; `charter-transport/src/relay_config.rs`
(`RelayConfig` has `deny_unknown_fields`), `ui/AboutActivity.kt`.

**Question:** RELAYS today reaches a ward only as a pasted code (Linux
`charter-pair --relay-code`, Android "Change relays"). It is not delivered
over relays or the local link. Is the PIN meant to travel the same way, or
should RELAYS also be accepted gift-wrapped?

**Build's interim reading (replaced after the batch 1 review):**
`guardianPin` is accepted only from a RELAYS event delivered gift-wrapped
(over relays or the local link) by a signer. A pasted relay code carrying
`guardianPin` is refused whole (`pin_not_wrapped`); a pasted code without one
works as today. The reference's `applyRelays` takes the channel
(`wrapped` | `pasted`) and enforces this; vector `relays_guardian_pin`.

**Review argument (M5), for the design session:** the PIN record is kept in
Device Owner private storage so the child can't read it. Pasting the RELAYS
JSON at the ward puts `{ salt, hash }` on the ward's screen and clipboard
(keyboard clipboard history). A 4–6 digit PIN under scrypt N=2^15 is
brute-forced offline in hours. Beyond the channel, the design session should
decide whether a longer PIN or passphrase is required, and state in the
contract everything the PIN gates besides presence tokens.

**Raised by:** batch 2 (planning).

## Removal notice measured from receipt

**Where:** rule 7 ("takes effect at the later of the confirmation and
`issuedAt` plus 72 hours"); `applyPresenceConfirm`.

**Question:** `issuedAt` is chosen by the remover and statements have no
floor, so a removal backdated by 72 hours applies the moment it is confirmed:
the removed parent never sees it pending and can't object. Is the notice
meant to run from `issuedAt`?

**Build's interim reading:** the notice runs from the later of `issuedAt` and
the ward's own receipt: `takesEffectAt = max(confirmedAt, max(issuedAt,
receivedAt) + 72 h)`. `receivedAt` is persisted with the pending statement and
projected. The contract wording needs the same fix. Vector
`removal_notice_from_receipt`.

**Raised by:** batch 1 review.

## Clause floor after a departure

**Where:** rule 4 ("compared only with clauses of that `(subject, kind)`
still held from current signers … a relay can't restore an older rule from a
current signer"); `acceptClause`, `clampClauses`.

**Question:** once a departed key's clause is the one held, nothing from a
current signer is held for that `(subject, kind)`, so a relay can replay any
older clause from a current signer (issued after pairing) over it, rolling the
rule back. The contract's wording admits this.

**Build's interim reading:** the per-`(subject, kind)` floor survives the
departure. When a signer departs, its held clause gets `floor = min(issuedAt,
ward time of the departure)`. While its signer is departed, a replacement must
be newer than the floor; while its signer is current, newer than its
`issuedAt`. A far-future clause from a departed key is thus floored at the
departure time, and a live guardian's fresh clause clears it. Vector
`clause_replay_after_departure`.

**Raised by:** batch 1 review.

## Removal re-issued after an objection

**Where:** rule 7 ("Neither parent can remove the other against their
objection"); `objectedBy`.

**Question:** an objection drops one removal. Nothing stops R1 sending a new
removal one second later, so R2 must object again and again. What does an
objection block?

**Build's interim reading:** an objection by R2 naming R1 blocks every
removal of R2 signed by R1 that reaches the ward within 7 days of the
objection reaching it (`objected`), on arrival, at confirmation and when its
timer falls due. After that a newer removal is pending as usual. The design
session may prefer to tie this to a relationship change instead of a time.
Vector `removal_reissued_after_objection`.

**Raised by:** batch 1 review.

## A moving root escapes a pending removal

**Where:** rules 7 and 8; `applyPresenceConfirm` (a confirmed move drops
pending statements naming the moved root).

**Question:** R1 has a confirmed removal of R2 pending. R2 moves to R3 and
confirms the move before the notice ends. The removal named R2, which is gone,
so it is dropped, and R3 is pinned with R2's authority. Should the removal
carry over to R3?

**Build's interim reading:** unchanged: the removal is dropped with R2. This
is no worse than an objection (R2 could have objected), but the design session
should decide whether a pending removal follows a move. Design session only;
no code change.

**Raised by:** batch 1 review.

## An old objection re-arms the removal block

**Where:** rule 7 ("Neither parent can remove the other against their
objection"); `src/guardians/state.ts` `processObjection` and `objectedBy`.

**Question:** an objection has no freshness bound. An objection signed long
ago but never delivered, then delivered now, drops R1's fresh removal
(`objection_applied`) and blocks re-issues for 7 days from its *receipt*. The
exposure is narrow (the seal is per ward, so the old wrap must have been
addressed to this ward and never processed, and R2 could object afresh
anyway), but it matters when R2's key is lost and R1 is removing an absent
co-guardian: each stray wrap buys another week. Should the 7-day block run
from `max(issuedAt, receivedAt)` clamped to `issuedAt + 7 d`, or should an
objection count only if its `issuedAt` is within 7 days of the pending
removal's?

**Build's interim reading:** unchanged: the block runs for 7 days from the
objection's receipt, however old its `issuedAt`. No code change. Design
session only.

**Raised by:** batch 1 re-review.

## Offline unlock with more than one guardian key

**Where:** *Who may sign what*, *Other places that name the guardian*;
`charter-crypto/src/unlock.rs` (the code is derived from the NIP-44
conversation key between the machine and the pinned guardian).

**Question:** the contract doesn't say which keys can answer an offline
unlock challenge once a ward has working keys. A root held on a Heartwood
cannot do ECDH over NIP-46, so a root-only answer would leave such a family
with no offline unlock.

**Build's interim reading:** a code is accepted if it matches the
conversation key with any key currently in the signer set
(`code_matches_any`). This is not fail-open: every key in the set can already
sign a CLAUSE or GRANT that lifts the lock, and a departed key is no longer
in the set. Linux still checks the pinned guardian's code only until the
platform batch wires the set through.

**Raised by:** batch 2a.

## A remote unpair with more than one root

**Where:** *Other places that name the guardian* ("the re-pairing authority
[is] any key in the signer set"), rule 7 ("Neither parent can remove the
other against their objection", "The last root leaves only by unpairing"),
rule 11; `charter-spine` `Broker::on_release`.

**Question:** a RELEASE unpairs the ward, dropping every root. With two roots
pinned, one parent's RELEASE would remove the other parent's control without
notice or objection. Is a remote RELEASE meant to be allowed then?

**Build's interim reading:** a RELEASE from any key in the signer set applies
only while the ward has exactly one root (today's behaviour). With more than
one root it is refused; a parent first removes the other root (rule 7), or an
administrator unpairs locally.

**Raised by:** batch 2a.

## Rule 4 for the machine-wide content clause

**Where:** rules 4 and 5 ("clauses of the same `(subject, kind)`");
`charter-spine` broker `ClauseSlot::Machine`, `charter-sys` `ClauseStore`.

**Question:** a `content` clause with no `subject` is machine-wide: it has no
`(subject, kind)` slot. Does the departure rule (and recording the signer)
apply to it?

**Build's interim reading:** not yet. The machine-wide slot keeps its replay
floor only and records no signer, so a departed key's `content` clause is
replaced only by a newer one from a current signer. That is fail-closed for
rollback; the far-future case it leaves open is bounded by the 300-second
future refusal. Per-child `content` clauses (with a `subject`) follow rules 4
and 5.

**Raised by:** batch 2a.

## A PAIR_OFFER whose inner event is not signed

**Where:** *Scan-to-pair with a working key*; `charter-guardians`
`pair_from_offer` (and the reference's `pairFromOffer`), which verify the inner
kind-31117 event's id and signature; `CharterTransport::send_pair_offer`, which
sends it as an unsigned rumor; `charter-spine` `scan_to_pair`.

**Question:** the guardian app signs its PAIR_OFFER inner event, but the Rust
guardian side does not (the seal alone authenticates it, as for a rumor). Must
the inner offer be signed?

**Build's interim reading:** a signed inner offer goes through
`pair_from_offer` in full. An unsigned one gets the same token checks and pins
its seal author, but only when it carries no `approval`: an offer with an
approval needs a signed inner event, and is refused otherwise (fail closed).

**Raised by:** batch 2c-A.

## An unreadable guardian-keys state

**Where:** rule 10; `charter-spine` `GuardianKeys::load`.

**Question:** the ward's guardian-keys state file is there but cannot be read
or parsed. Migrating afresh from the pairing record would re-admit a pinned
root that has since been removed or moved. What should the ward do?

**Build's interim reading:** fail closed. The signer set is empty, so nothing
signed is accepted and no local-link handshake completes. There are no
recipients, so nothing is encrypted to a key that may have departed. The
stored clauses stay enforced. The file is left as it is; a read error is
retried at each timer advance. Recovery is a local unpair by an administrator
(or restoring the file). The guardian sees the ward go silent: the design
session may prefer a STATUS to the last known roots saying the ward is stuck.

The same holds for a readable state that belongs to neither this pairing nor
an earlier one: the state records the guardian the pairing record pinned
beside `pairedAt`, and a state with the record's `pairedAt` but another
guardian, or a `pairedAt` above the record's, is unavailable rather than
migrated afresh (a fresh migration would re-pin the record's guardian over a
confirmed root move or removal). A state from an earlier pairing (a lower
`pairedAt`) is still replaced, and every re-pair path clears the state
before it writes the new record.

**Raised by:** batch 2c-A; the second paragraph by the batch 2 review.

## The clause pairing floor on a ward that paired before the section

**Where:** *Who may sign what* ("All existing checks (… pairing floor …) still
apply"); `accept_clause` (refuses a CLAUSE issued before `paired_at`);
`GuardianKeys::load`.

**Question:** a ward that paired before the section never had a pairing floor
for clauses, only the replay floor. Its migrated state takes `paired_at` from
the pairing record, so from the upgrade on it refuses a clause issued before
the current pairing. Is that intended? (It also refuses, on any ward, a clause
signed by a guardian whose clock was behind the ward's when they paired.)

**Build's interim reading:** yes, as the contract and the vectors have it: a
clause from before the current pairing belongs to an earlier one. The slots a
migrated ward already holds keep their own replay floors: the state seeds
each slot from the clause store the first time a clause for it arrives.

A seeded clause whose signer is no longer in the signer set departed before
the slot was seeded, so no departure clamped it. It takes rule 4's floor
instead: min(its `issuedAt`, the departure), where the departure is the
removal or move of a departed root, or the tombstone of a withdrawn or
removed working key; when the state does not record when the signer left,
the ward's time at seeding. (An earlier build kept the clause's own
`issuedAt` as the bar, calling it fail-closed. It was not: a departed root's
far-future clause, possible before the section's 300-second refusal, then
held the slot for ever, and the remaining guardians could not replace a
permissive rule.)

**Raised by:** batch 2c-A; corrected by the batch 2 review.

## One presence token at a time

**Where:** *Presence confirmation*; `register_presence_token`,
`GuardianKeys::mint_presence_token`.

**Question:** the state can hold several live tokens. May a ward show more
than one code at once?

**Build's interim reading:** no. Minting a token clears every earlier one, so
the code on screen is the only one that works. Tokens live in the `0600`
guardian-keys state and are spent by the confirmation in the same write; a
display copy may also be kept in the presence-token file, which the
confirmation takes down if it still holds that token.

**Raised by:** batch 2c-A.

## Two records of the relay list

**Where:** rule 11; the pairing record's `relays` (what the transport reads,
changed outside the broker by a pasted relay code) and the guardian-keys
state's `relays` and `previousRelays`.

**Question:** which one is authoritative, and does a pasted relay code start
the 7-day previous-relays window too?

**Build's interim reading:** the pairing record. A gift-wrapped RELAYS must
pass both the pairing record's checks and the state's `apply_relays`, and is
recorded in the state (with any guardian PIN) before the pairing record is
saved. When the adapter loads and finds the pairing's list differs from the
state's (a pasted code), it records that as a change, so the window covers it.

**Raised by:** batch 2c-A.

## Telling a guardian that the ward's clock is behind

**Where:** *Who may sign what* (the 300-second future refusal); the CLAUSE
ACK and STATUS counters.

**Question:** a ward whose clock is more than 300 seconds behind refuses every
new clause, tightening ones included, and the guardian sees a plain `refused`.
Android locks its clock; a Linux ward before NTP, or with its clock set back
while offline, does not. Should the ACK and STATUS name this refusal?

**Build's interim reading:** the refusal stays, as the contract requires; it
is reported as `refused`, like any other. Batch 2a review, L1.

**Raised by:** batch 2c-A.

## Presence code encoding (build-defined)

**Where:** *Presence confirmation* ("the ward then shows the one-time token as
a code to scan, with a short form to type"); Linux `charter-presence`, the
Android presence screen, the guardian app's scanner.

**Question:** the contract doesn't give the encoding of the presence code.

**Build's interim reading:** mirror the scan-to-pair invite
(`charter://pair?m=<64hex>&t=<32hex>`):
`charter://presence?m=<machine 64hex>&t=<token 32hex>&d=<pending d, percent-encoded>&s=<pending statement event id 64hex>`.
The code carries the statement's event id because the confirmation must name
it (`statement: <event id>`), and a new parent's install has never seen the
statement.
The short form to type is the token alone, 32 hex shown as four groups of 8;
the guardian app asks which device and which pending statement it is for.
The code is not a secret beyond its 600-second, single-use life, and is
worthless without a working key bound to the right root.

**Raised by:** batch 2c (planning).

## A clause that arrives with no seal

**Where:** *Who may sign what*; Android `charterIngestClause` (a bare signed
CLAUSE, no gift wrap), `Warden::ingest_clause`.

**Question:** the guardian-keys rules check a clause's seal author against
its signer. The Android JNI entry `charterIngestClause` takes a bare signed
event (a test and onboarding path with no production caller). What is its
seal author?

**Build's interim reading:** the event's own author stands in for the seal,
and the clause then goes through the same guardian-keys gate as a polled
one (signer set, rules 4 and 5, the 300-second refusal, the pairing floor).
Nothing reaches the clause store around the gate.

**Raised by:** batch 2c-C.

## Labels for roots

**Where:** rule 5 ("Each label is shown next to the signer's root identity");
Android About's guardian list, `charterGuardiansSummary`.

**Question:** labels ride approvals (a working key's) and co-guardian
statements (the new root's). A root pinned at pairing has no label of its
own. What does the ward show for it?

**Build's interim reading:** a root is shown as its label from the
co-guardian statement that pinned it, if any, else "A guardian", always
beside its short npub; its working keys' labels are listed beneath. A label
is never shown without the npub. Both platforms strip control characters
(newlines included), invisible format characters (bidi overrides and
isolates, zero-width marks) and line separators from a label before showing
it, and cut it to 64 bytes, so a label can't fake a line or an npub of its
own.

**Raised by:** batch 2c-C; label cleaning by the batch 2 review.

## Which pending statement the Android presence screen offers

**Where:** *Presence confirmation*; open question "Presence confirmation
details" (a token may be bound to one pending statement or left open);
Android Guardian presence screen.

**Question:** the core can mint an open token (`d` = none) that confirms any
pending statement. Should a ward's screen ever show one?

**Build's interim reading:** no. The Android screen lists the unconfirmed
pending statements and mints a token bound to the one the guardian picks;
with nothing pending it mints nothing. A removal already confirmed and
waiting out its notice is not offered again. The native call refuses a
request that names no statement, or a confirmed one, so this does not rest
on the screen alone; Linux `charter-presence` offers the same list.

**Raised by:** batch 2c-C.

## appVersionCode is per platform

**Where:** *Versioning* ("every ward of that child reports `appVersionCode`
at or above the first version that implements this section"); Linux
`charterd::version::version_code`, the Android `versionCode`.

**Question:** the two platforms number their builds differently. Linux
sends `major * 10000 + minor * 100 + patch` of the package version (0.7.12
is 712); Android sends its own `versionCode`. One threshold cannot serve
both, and STATUS carries no platform field to key a per-platform threshold
on.

**Build's interim reading:** Linux keeps sending the code it already sends
(no change). The guardian app must not offer co-guardians or root moves on
`appVersionCode` alone until the contract records one threshold per
platform and says how the guardian tells the platforms apart (the device
record's platform from pairing, or a new STATUS field). Until then it
should offer them to no child with a ward it cannot place (fail closed).

The guardian app keys a per-platform threshold on the device record's
platform, and counts a ward it has not heard an `appVersionCode` from as not
ready (never "silence means capable"); the wards are unchanged.

**Raised by:** batch 2c-B; the last paragraph by the batch 2 review.

## How a Linux administrator mints a presence token

**Where:** *Presence confirmation* ("an administrator on Linux (polkit, as
for other admin actions)"); `linux/crates/charterd/src/guardians.rs`,
`charter-presence`, `charter-console`.

**Question:** the contract says who may mint, not how the request reaches
the warden, which is the one process that may write the guardian-keys state.

**Build's interim reading:** `charter-presence` runs as root (pkexec from the
console's "Confirm in person", or sudo). It lists what is pending by reading
the state without writing it, writes a `0600` request `{d, requestedAt}` into
the root-only state directory, and waits up to 15 seconds. `charterd` takes
the request on its next 2-second tick, refuses one older than 120 seconds
or dated in the future, mints a token bound to that `d` (refused when
nothing with that `d` is pending), records it in the state and writes the
`0600` display copy, then removes the request. The helper shows the code
from the display copy. Nothing pending: the helper says so and writes no
request.

**Raised by:** batch 2c-B.

## The pairing record's guardian key after a root move

**Where:** rule 8; the pairing record's `guardian_pubkey`;
`charter-spine` guardian-keys module docs.

**Question:** after a confirmed move from R to R2, should the pairing
record's `guardian_pubkey` become R2?

**Build's interim reading:** no. The guardian-keys state is authoritative;
the record's field stays the root pinned at pairing. It is read only as the
signer of a clause stored before the section (rule 5) and as the
transport's legacy single recipient when no signer set is wired. Rewriting
it would attribute R's old clauses to a current signer and lift their
departure floor (rule 4). The Linux CLI and console show the roots from
the warden's published view instead.

**Raised by:** batch 2c-B.

## Gift-wrap timestamps link a ward's fan-out

**Where:** rule 9 (one wrap per recipient); `CharterTransport` seal and wrap
`created_at` (review 2a, Info 3).

**Question:** every wrap of one STATUS carries the same `created_at`, so a
relay can group a ward's wraps and learn all of its guardians' working keys.
NIP-59 backdates `created_at` by a random amount instead.

**Build's interim reading:** unchanged. The guardian app reads with a
`since` cursor whose floor is 24 hours, and the wards poll with a lookback;
wraps backdated by up to two days would be stepped over. Jitter needs the
readers' windows widened first, on every platform at once. Design session.

**Raised by:** batch 2c-B.

## Offline unlock codes and the lock's back-off

**Where:** open question "Offline unlock with more than one guardian key";
`charter-lock` (review 2a, Info 7).

**Question:** accepting a code from any of up to 68 keys makes a guess up to
68 times as likely to hit.

**Build's interim reading:** charterd hands the lock one code per key in the
signer set; the lock's cooldown cap is 300 seconds times the number of codes
(30, 60, 120, … seconds as before, then the longer cap), so a sustained
guesser gets the same odds per hour as against one code. An empty set (an
unreadable state) gives no codes, and the lock keeps its device-only escape.

**Raised by:** batch 2c-B.

## A departed key's last STATUS

**Where:** *STATUS* ("a departed key receives one last STATUS that shows its
departure"); `GuardianKeys::farewell`; charterd's STATUS batch; the Android
poll.

**Question:** what does that last STATUS carry, and when is the key told?
The STATUS the signer set's recipients get holds the child's usage, apps and
lock state, and the ward's current relay list (`guardians.relays`). For a
removed root, that is more than "shows its departure". A further change can
also land while a STATUS is publishing.

**Build's interim reading:** the departed key gets a separate, minimal
STATUS (kind 31114, gift-wrapped to it alone, on the current relays and any
previous ones of rule 11), never the full one. Its content is exactly:

```json
{ "v": 1, "machine": "<hex>", "ts": 1700000000, "seq": 42,
  "guardians": { "roots": [{ "root": "<hex>", "keys": ["<hex>"] }],
                 "pending": [{ "d": "...", "kind": "removal",
                               "takesEffectAt": 1700000000,
                               "expiresAt": 1700000000 }],
                 "tombstones": 3 } }
```

No `subject`, no usage, apps or lock state, and no `relays`; the departure
shows as the key's absence from `roots`. `seq` comes from the ward's one STATUS
sequence. The guardian app must take this shape as a farewell rather than
drop it as a malformed STATUS.

The ward reads the keys owed a farewell and the `guardians` it shows in one
step, sends each key its own wrap, and marks as told only the keys whose own
wrap a relay accepted; a key that departs meanwhile stays owed and is told
alongside the next STATUS. A farewell is never parked in the Android
offline spool: the state keeps it owed. A STATUS parked for a key before it
departed is dropped, never sent or re-addressed, and so is a break-glass
AUDIT wrap journaled for it (the audit is built afresh for the current
recipients if none of its wraps is left). A local-link copy addressed to a
departed key is not held: its session is closed (review 2a, Info 5).

**Raised by:** batch 2c-B; rewritten by the batch 2 review.

## Who may see a pending removal on the ward

**Where:** rule 5; rule 7 ("reported in STATUS to R2's working keys");
charterd's world-readable `/run/charter/guardians.json`.

**Question:** the ward's own CLI and console show who set each rule, and the
console must list what is waiting to be confirmed in person. Is it right
that the child can see that one parent has asked to remove the other?

**Build's interim reading:** yes, as the ward can see everything enforced on
it: the view lists the pending statement's kind, the short npub it names,
and when it takes effect or expires; a label never appears without its
root. A design session may prefer to hide removals from the ward's own
screen.

**Raised by:** batch 2c-B.

## The roster's subject for a child with no dependant key

**Where:** *Family roster* (`subject` per child); `mergeRoster`'s 64-hex
`subject` check; the guardian app's `Child.dependantPubkey` (always null) and
`Child.rosterId`.

**Question:** a guardian app child has no dependant pubkey, and each ward
mints its own subject. What does a roster record use as `subject`?

**Build's interim reading:** each child gets a random 32-byte `rosterId`
(64 hex), minted once by the install that created the child and kept. It is
not a Nostr key and never goes to a ward. An install that adopts a child from
a roster keeps the sender's `rosterId`, so records for the same child merge.

**Raised by:** batch 3a.

## The objection's p when STATUS doesn't name the remover

**Where:** statement table (objection `p` = the removal's signer); STATUS
`pending` (`{ d, kind, takesEffectAt?, expiresAt }`, no signer); guardian app
`objectToRemoval`.

**Question:** a removed parent's install learns of a pending removal only
from STATUS, which doesn't say who signed it. Which key goes in `p`?

**Build's interim reading:** the caller may name the remover. Otherwise the
app takes the one root other than its own that the wards reporting that
pending `d` list, and refuses when there is more than one (fail closed: an
objection with the wrong `p` would be held, not applied). A STATUS `signer`
on pending entries would remove the guess.

**Raised by:** batch 3a.

## Rule 4 when a parent leaves

**Where:** rule 4 ("A guardian app re-signs the current clause set with a
live key before it sends a departure"); guardian app `leaveFamily`.

**Question:** a parent removing their own root departs with every key their
install holds, so it has no live key to re-sign with.

**Build's interim reading:** `leaveFamily` does not re-sign. The clauses it
set stay enforced (rule 4) until the remaining parent sends a clause of the
same kind. Removing another parent, withdrawing an install and a root move
do re-sign first, and abort if the re-sign can't be sent:
- withdrawing this install's own working key re-signs with the root R, never
  with W (W is the key departing);
- the re-sign covers every clause kind the install sends to the child's
  devices: the policy clauses, and the latest gift, stand-down, maintenance
  window, update and debugging clause per device (a gift keeps its `id`, so
  it is never added twice);
- it counts only once sent: a clause held for the home network aborts the
  departure, which could otherwise apply before the re-sign arrives;
- a kind this install never sent is not re-signed; a departed key's clause
  of that kind stays enforced until someone sends one.

**Raised by:** batch 3a; widened by the batch 3a review.

## A working-key PAIR_OFFER to a ward that predates the section

**Where:** *Scan-to-pair with a working key*; `charter-transport`
`PairOffer` (an older build has no `approval` field and doesn't refuse unknown
fields); guardian app `sendPairOffer`.

**Question:** with a Signet root, the app seals the offer with W and adds the
approval. A Linux ward older than the section ignores `approval` and pins W
as its guardian, not R; the app then signs that ward's events with R (no
`guardians` in its STATUS) and the ward refuses them.

**Build's interim reading:** the app never sends a W-sealed offer. Under a
Signet root, scan-to-pair is signed and sealed by R over NIP-46 with no
approval, so an old ward and a new one both pin R, exactly as before the
section. W's approval then follows as for any other ward ("When the guardian
app sends W's approval"): at once, because the app can't read the new ward's
STATUS (it goes to R) until the approval has landed; an old ward drops it
harmlessly. W signs that ward's events only once its STATUS lists W under R
and the child passes the versioning gate.

The earlier reading (send the W-sealed offer and rely on releases shipping
the scanner only with section-implementing wards) failed open: nothing in
the code stopped it, and an old Linux ward then pinned W while the app
signed it with R, so it refused everything. *Scan-to-pair with a working
key* stays unused until the pairing code carries a version marker (design
session).

**Raised by:** batch 3a; reading changed after the batch 3a review.

## Presence code carries no relays

**Where:** *Presence confirmation*; the build-defined presence code; guardian
app `confirmPresence`.

**Question:** a joining parent's install has never paired the ward and isn't
yet one of its STATUS recipients. Where does it send the confirmation?

**Build's interim reading:** to its default relays, plus every relay it knows
for that machine (its pairing and the ward's reported `guardians.relays`),
and over the home network where the carrier has a route. A ward on custom
relays only can't be confirmed from a new install until the code carries a
relay hint.

**Raised by:** batch 3a.

## Connecting My Signet over devices that trust this phone's key

**Where:** *Versioning*, rule 8; guardian app `approveInstall`.

**Question:** a local-only family's wards pin the phone's own key. If that
install then connects My Signet as root, R's approval is for a root that
isn't pinned (dropped) and anything R signs is refused.

**Build's interim reading:** the app refuses to approve the working key while
any paired device may trust the local key, naming those devices. Moving them
to My Signet is a root move. Each device records the key it was paired to
(`pinnedKey`). Older records are stamped when the answer is still known:
- before a phone that signed with its own key (or had no signer) connects
  My Signet, every device without a record is stamped with the local key;
- when a family already on My Signet first approves this phone, devices it
  has never heard from are stamped with R (their STATUS goes to R).
A device counts unless its STATUS lists the local key as a working key, or
its record names another key. A device with no record counts whether or not
it has been heard from (fail-closed).

**Raised by:** batch 3a; tightened after the batch 3a review (it skipped
devices not heard from).

## Moving a phone-held root

**Where:** rule 8; open question "Presence confirmation details" (the
confirming key must be a working key, `key_role_conflict`); guardian app
`moveRoot`.

**Question:** in a local-only family the install's key is the root. After a
move to R2 it would have to become R2's working key, a role change the
reference refuses.

**Build's interim reading:** `moveRoot` is refused for a local-only family.
Only an install with a My Signet or Heartwood root can move it. A local
family that wants My Signet waits for a design decision (a fresh working key
on the phone, or a re-pair).

**Raised by:** batch 3a.

## Android pairing under a Signet root

**Where:** open question "Android pairing has no PAIR_OFFER"; guardian app
phone pairing (STATUS `pairToken` auto-claim).

**Question:** the bunker link names R, so the new ward sends STATUS only to R
until W's approval lands, and W can't send its approval until it knows the
machine key. The guardian app never sees the `pairToken` echo.

**Build's interim reading:** the app names R in the link and SAS as the
interim reading says, but it only shows the phone QR while it holds a local
signer, as before. Under a Signet root, a ward phone is claimed by its device
code (the existing manual path); the approval goes out as soon as the device
is recorded. A carried W (or approval) in the link, covered by the SAS, would
close the gap.

**Raised by:** batch 3a.

## A phone-held root has no approval for the roster

**Where:** *Family roster* ("carries the sender's approval"); `mergeRoster`
(`bad_approval`); guardian app `sendRoster`.

**Question:** an install whose key is itself the root signs directly and has
no approval. What does its roster carry?

**Build's interim reading:** it sends no roster. Only an install with a
My Signet or Heartwood root shares the roster. The other parent's install
learns the family from its own pairings and STATUS.

**Raised by:** batch 3a.

## The key backup copies the working key

**Where:** *Terms* ("Working key (W): one per guardian install, generated
there and never copied"); guardian app key backup and restore.

**Question:** the app's encrypted key backup restores the local key, which is
now W, onto another phone, so two installs share one W.

**Build's interim reading:** unchanged for now: the backup exists so a
local-only family (whose key is its root) survives a lost phone. With a
Signet root, a restored phone should instead get a fresh W approved by R, and
the lost W withdrawn. Design session.

**Raised by:** batch 3a.

## When the guardian app sends W's approval

**Where:** *Delivery and the two seal exceptions* ("A W sends its own
approval to a ward before its first signed event"); guardian app
`syncApprovals`, `keyFor`.

**Question:** an old ward refuses kind 31120, and a new ward's STATUS goes to
R until the approval lands, so the app can't tell which is which first.

**Build's interim reading:** the app sends the approval (sealed by W) to every
paired ward that doesn't list W under R: at once on connecting the root, and
again at most every 6 hours while the ward doesn't list it. W signs a ward's
events only once that ward's STATUS lists W under R and every ward of the
child passes the versioning gate (thresholds Android 49, Linux 713, keyed on
the platform the device was paired as); until then R signs, so W never signs
before its approval has landed.

**Raised by:** batch 3a.

## A departed key's farewell in the guardian app

**Where:** open question "A departed key's last STATUS"; guardian app
`unwrapFarewell`, `applyFarewell`.

**Question:** what the guardian install does with the farewell.

**Build's interim reading:** it is authenticated like a STATUS (machine
seal, rumor author = `machine`) and never read as usage. When this install's
W is neither a root nor any root's working key in it, the device is marked
departed ("This phone no longer has access to …") and nothing more is sent to
it. A farewell older than the view already held, or one that still lists W,
changes nothing. A later full STATUS to this install clears the mark.

**Raised by:** batch 3a.

## The guardian PIN's length in the app

**Where:** *Presence confirmation*, the Android guardian PIN; open question
"How a guardian PIN reaches an Android ward" (review M5).

**Question:** the contract sets no length.

**Build's interim reading:** the guardian app asks for at least 6 digits and
sends only to Android wards at or above the versioning threshold (an older
ward refuses an unknown RELAYS field outright). The derivation matches
`vectors/guardians/pin.json`, including its 4-digit vector.

**Raised by:** batch 3a.

## The typed presence code needs the statement's id

**Where:** *Presence confirmation* (the confirmation names the pending
statement's event id); STATUS `pending` (`{ d, kind, takesEffectAt?,
expiresAt }`, no id); guardian app presence scanner.

**Question:** the short form a guardian types is the token alone. The app
must then supply the statement id, which STATUS doesn't carry.

**Build's interim reading:** after the typed token the app offers only the
pending items this install sent (it kept their ids: a removal, or its own
root move), per device. A joining parent confirming an invitation must scan
the full code. A STATUS `id` on pending entries would let the short form
cover invitations.

**Raised by:** batch 3b.

## Whether a ward phone has a guardian PIN

**Where:** *Presence confirmation* ("The guardian app sets one before it
offers anything that needs presence"); guardian app `pinSentAt`.

**Question:** STATUS doesn't say whether a PIN is set, and each install only
knows the PINs it sent. A second guardian's install would ask to set a PIN
the other parent already set, and replace it.

**Build's interim reading:** before inviting, removing, moving or confirming
in person, the app asks for a PIN when it hasn't sent one to every Android
ward itself. When the wards report more than one guardian it also offers
"The other parent already set a PIN", which skips it: a ward phone with no
PIN can't show a code, so a wrong skip fails closed at the ward. A STATUS
flag (`pinSet: true`) would remove the guess.

**Raised by:** batch 3b.

## Removing the other parent from a phone-held root

**Where:** rule 7 (presence by an install bound to R1); `applyPresenceConfirm`
(`confirmer_not_bound`: the confirmer must be an active working key bound to
the remover); guardian app `removeCoGuardian`, `confirmPresence`.

**Question:** in a local-only family the install's key is the root itself,
not a working key bound to it, so its confirmation of its own removal
statement is dropped. The removal could never complete.

**Build's interim reading:** the app offers "Remove the other parent" only
with a My Signet or Heartwood root, and says why otherwise. `removeCoGuardian`
and `confirmPresence` themselves don't refuse it yet. Inviting stays open to
a local-only family: the new parent confirms with their own working key.

**Raised by:** batch 3b.

## Checking the invited identity when joining

**Where:** guardian app join flow; `pairSignet` (approves this phone as soon
as the bunker connects); `approveInstall({ expectRoot })`.

**Question:** the joining parent should connect the identity the other
parent invited. `pairSignet` asks the bunker to approve the working key
before the app can compare identities.

**Build's interim reading:** on a phone already connected to My Signet but
not approved, the app takes the invited npub (optional) and passes it as
`expectRoot`. On a fresh connection it shows the full npub after approval
and says the devices won't accept a different one; the app's own
`confirmPresence` refuses an invitation for another identity, so a mismatch
fails closed. Passing `expectRoot` through `pairSignet` (refuse and
disconnect before approving) would check it up front.

**Raised by:** batch 3b.

## Guardians have no names in STATUS

**Where:** STATUS `guardians.roots` (`{ root, keys }`); statement labels.

**Question:** the Guardians screen lists each root and its installs, but
STATUS carries no label for either, and a joining install never sees the
statements that carry them.

**Build's interim reading:** roots show as "You" or "Other parent" with a
short npub, installs as "Phone" and the first 8 hex of the working key, with
"this phone" marked. No label is shown that a ward didn't vouch for.

**Raised by:** batch 3b.

## A joining parent's device list

**Where:** *Family roster* (sent to the working keys the wards report);
guardian app join checklist.

**Question:** the inviting install shares the roster only with working keys
the wards report, and a joining W is reported only after its first
confirmation. Before that the joining phone doesn't know which devices to
visit.

**Build's interim reading:** the join checklist is generic (go to each
device, ask it for a code, scan it), and lists roster devices as "still to
confirm" once a roster has arrived. The inviting parent's checklist tells
them to name the devices.

**Raised by:** batch 3b.

## A root move that lapses half-way

**Where:** rule 8; *Pending statements*; guardian app `moveRoot`,
`settleRootMove`.

**Question:** a move to R2 takes effect per ward, on presence. If it is
confirmed at some wards and then expires or is cancelled, those wards pin R2
and the rest still pin R. The contract has no state for "half moved".

**Build's interim reading:** each ward is judged by its own STATUS:
- a ward that pins R2 and lists W under it gets W (and R2's approval if it
  lacks W); a ward still on R gets W under R or R itself, as before;
- the move is forgotten once it has expired (by the wards' STATUS `pending`,
  else 7 days after `issuedAt`) or this install cancelled it, but only when
  no ward pins R2; otherwise R2 is kept, so the moved wards aren't stranded;
- while any ward is half-way, statements signed by R (co-guardian, removal,
  withdrawal, leaving, a move elsewhere) are refused, since the moved wards
  would drop them. Restarting the move to the same identity finishes it.
Signing those statements with R2 for the moved wards needs R2's bunker as a
second live signer; design session.

**Raised by:** batch 3a review.

## Who decides which roots may send a roster

**Where:** *Family roster* ("An install accepts a roster only from a W
approved by a root that the family's wards report in STATUS"); guardian app
`reportedRoots`, `sendRoster`.

**Question:** the roots are the union of every ward's STATUS. One tampered
ward (a rooted phone, root on a laptop) can report an extra root and working
key; the app then sends every child's name, colour and device list to that
key and accepts its rosters. Roster data never reaches the family itself
(`children`), but the names leave the family.

**Build's interim reading:** unchanged, matching the contract's wording.
Possible hardening for the design session: count a root only when more than
one ward reports it, or when a ward this install paired reports it and an
approval for its working key has been seen; or ask the guardian before the
first roster goes to a new key.

**Raised by:** batch 3a review.

## A departed working key whose root is still pinned

**Where:** *STATUS* ("a departed key receives one last STATUS"); rule 9;
guardian app `applyFarewell`.

**Question:** when this install's own W is withdrawn, the ward sends W a last
STATUS. Its root R is still pinned there, so the install can still sign with
R, but the ward's later STATUS goes to R, which the app never reads.

**Build's interim reading:** the device is marked departed only when none of
this install's roots is pinned in the farewell. Otherwise the view is
recorded and R signs for it from then on; the app shows what it last heard.
Hearing from it again needs a fresh approval of a new working key.

**Raised by:** batch 3a review.
