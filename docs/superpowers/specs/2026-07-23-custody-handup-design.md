# Custody Hand-Up — Local Guardian Key → Signet Vault (Design)

**Date:** 2026-07-23
**Status:** Designed ahead of need (per D2: authority is the semi-sticky layer;
specify the migration before drift forces a rewrite). No implementation now.
**Parent:** `2026-07-22-approvals-architecture-face-and-vault-design.md` (D2/D4).

## The one invariant everything hangs on

**Hand-up transfers the SAME secret; the guardian pubkey never changes.**
Every paired device (warden, Device Owner, future Charter-aware apps) pins the
guardian pubkey. Because custody moves the secret rather than rotating the
key, the wire identity is untouched: **zero re-pairing, and no device ever
learns the hand-up happened.** (Key *rotation* is a different, harder protocol
— out of scope here, becomes possible later via a vault-signed successor
announcement; do not conflate.)

## Two packagings, one flow

- **Embedded (Charter-only families):** the Signet custody component ships
  inside the Charter carrier APK. Hand-up moves the sk from the WebView's
  localStorage (+ the carrier's provision copy) into the component's
  Keystore-backed store, then retires the plain copies. No new app installed;
  the guardian sees a one-screen "Protect your key" step, not the word Signet.
- **Standalone (Nostr-native families):** hand-up rides the EXISTING
  encrypted key-backup format (`keyBackup.ts`) — the vault imports the same
  blob a device-recovery would, proves possession, and the surface retires
  its local copy. Reuse, not new crypto.

## The protocol: arm → prove → retire (never delete-then-hope)

1. **Arm.** Surface exports the encrypted blob (existing backup path) and
   hands it to the vault (in-process for embedded; QR/file for standalone).
   The local key REMAINS live and signing.
2. **Prove.** The vault must demonstrate possession + liveness: the surface
   issues a random nonce; the vault returns a BIP-340 signature over it; the
   surface verifies against the PINNED guardian pubkey. A vault answering
   with any other key is refused outright (this is the drift-guard, now
   load-bearing). The surface then routes ONE real signing operation (e.g. a
   STATUS-ack or a self-addressed test wrap) through the vault end-to-end.
3. **Retire.** Only after (2): the surface flips `custody: "local" → "vault"`
   in SignerState, deletes the localStorage nsec + carrier prefs copy, and
   records the hand-up (timestamped, in app state — auditable, not secret).
   The guardian's own encrypted backup (paper/file) is explicitly NOT
   deleted — it remains the disaster path and now restores INTO the vault.

Failure at any step leaves the local key authoritative — the flow is
re-runnable and abandonable; there is no half-state where nothing can sign.

## Surface changes (small, seams already exist)

- `SignerState.custody: "local" | "vault"` — drives which `GuardianOps` gets
  built: `localSigner.ts` (today) vs the vault-backed sibling
  (`signetSigner.ts` seam; `subscribeRequests.ts` already documents the
  NIP-46 decrypt variant).
- Decrypt path: the carrier service's classify needs the sk. Embedded: the
  component exposes decrypt-only ops to the service (key never leaves
  Keystore). Standalone: the carrier requests a session decrypt capability
  from Signet at provision **[decide with signet-app: scope + revocation of a
  "notification decryptor" grant — a deliberately weaker capability than
  signing, aligned with D3]**.
- D3 trust: the auto-sign ("vault signs its paired console's rulings") is
  granted AT hand-up, shown as one honest consent screen.

## Sequencing / dependencies

1. Blocked on the **embeddable-Signet component boundary** (face/vault
   follow-up #5) for the embedded packaging; the standalone path additionally
   waits on signet-app's bunker lane (contract Phase-10 items).
2. Until then the ONLY guardrail needed is the standing D2 discipline: no new
   custody features in MyCharter local mode (backup/restore stays as-is).
3. First implementation slice when unblocked: the **prove** step (nonce
   challenge + pinned-pubkey verify) — it is also the drift-guard MyCharter
   wants regardless of hand-up.

## Explicitly out of scope

Key rotation; multi-guardian custody (quorum vaults); ward-key custody moves
(dependant keys already live Signet-side by design); any UI beyond the two
consent screens named above.
