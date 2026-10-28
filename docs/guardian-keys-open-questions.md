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

**Build's interim reading:** the one the ward already holds stays; the
newcomer is dropped as `tie_not_newer`. Re-delivery of the held event itself is
`duplicate`. Only a strictly newer `issuedAt` replaces a held statement.

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

**Build's interim reading:** yes. The reference refuses any of them whose
timestamp is more than 300 seconds ahead, on top of their existing skew rules.

**Raised by:** batch 1.

## Cancel or objection before its statement

**Where:** *Pending statements* (cancel), rule 7 (objection);
`cancelledBy`, `objectedBy`, `processCancel`, `processObjection`.

**Question:** a cancel or objection can arrive before the statement it
targets (relays don't order delivery). Is it dropped, or held? If held, does
an objection kill only removals at or below its `issuedAt`, or any later
removal too?

**Build's interim reading:** both are held. When the target arrives, it is
dropped (`cancelled` / `objected`) if the held cancel was signed by the
target's signer, or the held objection by the root being removed naming the
removal's signer, and the target's `issuedAt` is at or below the held one's.
A newer target is unaffected, mirroring the cancel rule. An objection never
applies to a removal from a different signer than its `p`.

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
