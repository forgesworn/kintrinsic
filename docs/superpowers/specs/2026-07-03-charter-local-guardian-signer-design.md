# Charter — local-key guardian signer (MyCharter self-signs) — design

**Date:** 2026-07-03
**Status:** approved (design confirmed with decented); implementation pending
**Author:** lead engineer (Claude) + decented (founder / CEO-CTO)
**Implements:** D1 from `2026-07-02-charter-production-three-tier-design.md`
**Forcing function:** "Family Freedom Tech" talk, ~17 July 2026 — this is the last
substantial T2 (phone→laptop) code gap, and it's fully unblocked (zero Signet work).

## Goal

Give the MyCharter PWA its **own** guardian BIP-340 key so it can sign and
gift-wrap CLAUSEs directly — a real production signer, not a demo trick — via the
injection seam `realSigner.ts` already exposes. This closes T2 gap (a) without any
Signet-repo dependency. Signet remains the "bring your own identity" upgrade on the
same seam, wired later.

## The seam (why this is a clean drop-in)

`RealSigner` (`src/signer/realSigner.ts`) already takes an **injected**
`GuardianOps`:

```ts
interface GuardianOps {
  pubkey: string;                                            // the pinned guardian pubkey (hex)
  signEvent(t: EventTemplate): Promise<NostrEvent>;          // sign inner CLAUSE (31113)
  nip44Encrypt(recipient: string, plaintext: string): Promise<string>;  // seal/wrap encryption
}
```

Today `signetSigner.ts` builds that `GuardianOps` from a NIP-46 bunker. This
design adds a **second builder — `localSigner.ts`** — that implements the same
three ops directly from a locally-held key with `nostr-tools`
(`finalizeEvent` for `signEvent`, `nip44.encrypt` for `nip44Encrypt`).

**Nothing downstream changes.** The clause orchestration, NIP-59 gift-wrap
(`wire/giftwrap.ts`), relay publish/subscribe, and the STATUS feed are already
built and unit-tested against `GuardianOps` — they neither know nor care whether
the key sits in a bunker or in the browser.

## Zero device-side (charterd) changes

The device pins the guardian from a
`bunker://<guardian-pubkey>?relay=wss://…&kind=charter` string and thereafter only
**verifies inner signatures** against that pinned pubkey (`spec/contract.md`,
"Pairing — `bunker://` grammar"). It **never dials a bunker.** So a local-key PWA
simply exports **its own** pubkey as that same `bunker://…` string (+ QR) for
`charter-setup` to pin. The trust root is unchanged: the laptop still pins exactly
one guardian pubkey; only *which key signs* changes.

## Recovery & revocation model — re-pair, no backup secret

**Decision (confirmed with decented): no seed phrase, no passphrase.** The guardian
key is generated on first run and lives in the browser. Recovery of a lost/reset/
renewed phone is **re-pairing**, which the contract already gates to admin
("re-pairing requires admin or the existing guardian's signed authorization —
never the managed user").

Why this is the right call, not just the cheap one:

- **Re-pairing is both recovery *and* revocation.** A new phone generates a fresh
  guardian key; admin re-pins its pubkey on each device. That simultaneously
  restores control *and* renders a lost/stolen phone's old key powerless — the
  laptop no longer trusts it. A seed phrase gives no revocation and is itself
  stealable/photographable.
- **A seed phrase doesn't save the trip.** You wouldn't carry it, so recovery
  still means getting to the laptop — where you can just re-pair. It's pure added
  management burden for the target (non-technical) user.
- **Not fail-closed for the child.** *Verified in code:* the enforcer keys off the
  **authenticated, cached clauses** (`charter-schedule/src/enforcer.rs`); a
  guardian going quiet means no *new* clause arrives, so the **last good clause
  keeps being enforced**. The fail-safe lock fires only on a *malformed* clause,
  never on guardian absence. So a lost phone never bricks the kid's laptop — it
  keeps working under the last limits; only "grant more time / edit limits /
  approve a new app" pauses until re-pair. Recovery therefore carries **no
  urgency**.
- **Remote escape hatch for the technical, for free.** A power user who is away
  from the laptop can SSH / remote-desktop in as admin and re-pair. No code for us.

**Frequency:** phone loss/renewal is a once-every-few-years event, and the primary
flow — walk up to the laptop, re-pair — is a couple of minutes. The remote edge
case (guardian away from all devices, needs to change a limit *now*) is not worth a
seed phrase; it's covered adequately by the SSH/remote-desktop path.

**Optional, deferred:** because the key is regenerable, an encrypted key *export*
could later be offered as a power-user convenience on the same seam. **Not
required, not built for the talk.**

## At-rest protection

Store the key in the browser (localStorage/IndexedDB) and lean on the **phone's own
OS lock screen**. No app passphrase for v1. The theft window is bounded (the thief
would need an unlocked phone, and can only *loosen a child's limits*, not exfiltrate
data), and re-pairing revokes the stolen key immediately. A lightweight optional app
PIN can be added later if wanted; it is out of scope for the talk.

## Components to build

1. **`src/signer/localSigner.ts`** — `createLocalSigner(config)` returning a
   `RealSigner`. Generates the key on first run, persists it, and builds a
   `GuardianOps` from it (`finalizeEvent` / `nip44.encrypt`). `publish` uses the
   same `SimplePool` fan-out as `signetSigner.ts`. Mirrors `signetSigner.ts`'s
   shape so the two are obviously siblings.
2. **Key store** — a small persistence helper (get-or-create the guardian secret
   key in the browser; expose the pubkey). Single responsibility, unit-testable.
3. **`SignerKind` + store wiring** — add `"local"` to the `SignerKind` union
   (`domain/types.ts`) and a `"local"` branch in `store.tsx`'s `buildSigner`
   (alongside the existing `bunkerUri → Signet` / else-mock branches).
4. **"Pair this laptop" screen** — surface the PWA's guardian
   `bunker://<pubkey>?relay=…&kind=charter` string **and QR** for `charter-setup`
   to pin. This is the guardian→device half of pairing (the device→PWA half —
   scanning the device code — already exists).
5. **Re-runnable pairing** — confirm the existing pairing flow can be re-run on a
   fresh phone: re-scan each device's code (new phone learns device pubkeys) and
   admin re-pins the new guardian pubkey. Fix anything that assumes first-run only.

Items 1–3 are mechanically determined by the existing seam. Item 4 is net-new UI
(small). Item 5 is a verification/hardening pass on existing code.

## Data flow

```
first run:  generate guardian sk → persist → derive guardian pubkey
pairing (guardian→device):  PWA shows bunker://<guardianPubkey>?relay=…&kind=charter (+QR)
                            → admin runs charter-setup on the laptop → device pins guardianPubkey
pairing (device→PWA):       device shows its code → PWA scans → learns devicePubkey (already built)
sign:       parent changes a limit → policyToClauses → for each device:
            giftWrapClause(clause, devicePubkey, localGuardianOps) → publish to relay
verify:     device unwraps → re-derives inner id → verifies schnorr sig
            → checks author == pinned guardianPubkey → caches → enforces
recover:    lost phone → new phone regenerates key → admin re-pairs (re-pins new pubkey)
            → old key no longer trusted (revoked); child's laptop unaffected throughout
```

## Non-goals

- No seed phrase / recovery phrase.
- No app passphrase / PIN at rest (v1).
- No server-side / cloud account — MyCharter is local-first and ForgeSworn never
  custodies a family's guardian key (this is the sovereignty guarantee).
- No key export (deferred, optional, power-user only).
- No changes to charterd, the wire contract, gift-wrap, relay, or STATUS code.
- GRANT (ask-for-more-time / install) remote signing stays deferred (unchanged).

## Testing

- `localSigner` unit tests mirroring `realSigner.test.ts`: a generated key signs
  an inner CLAUSE whose sig verifies and whose author == the exported pubkey;
  `nip44Encrypt` round-trips; the emitted `bunker://…` string parses back to the
  same pubkey + relay.
- Key-store test: get-or-create is stable across reloads (same pubkey), and a
  cleared store regenerates a *different* key (the revocation property).
- Store test: `buildSigner` selects the local signer for a `"local"` signer state
  and produces a working `Signer`.
- Re-pair test: pairing state can be rebuilt from scratch on a fresh key without
  stale first-run assumptions.

## Open [decide] items

- Exact on-screen copy for the "Pair this laptop" step (normie-friendly; align with
  the in-app guide's voice and the wardship lexicon).
- Whether to show the guardian identity as `bunker://…`, `npub…`, or both on the
  pairing screen (device accepts hex/npub/nprofile; `bunker://` carries the relay +
  `kind=charter` the daemon pins — so `bunker://` is the load-bearing form).

## Sequencing

This is Track A work per the three-tier design. After it lands, the remaining T2
gaps are live-proving (real relay end-to-end + a live pairing round on hardware) —
gated on decented, not code. T1 bring-up runs in parallel on the hardware track.
