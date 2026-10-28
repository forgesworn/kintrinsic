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

**Raised by:** batch 2c-A.

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
migrated ward already holds keep their own replay floors (the state seeds
each slot from the clause store the first time a clause for it arrives; a
seeded clause whose signer has since departed keeps its replay floor, fail
closed).

**Raised by:** batch 2c-A.

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
`charter://presence?m=<machine 64hex>&t=<token 32hex>&d=<pending d, percent-encoded>`.
The short form to type is the token alone, 32 hex shown as four groups of 8;
the guardian app asks which device and which pending statement it is for.
The code is not a secret beyond its 600-second, single-use life, and is
worthless without a working key bound to the right root.

**Raised by:** batch 2c (planning).
