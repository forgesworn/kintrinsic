// Signed-inner NIP-59 gift-wrap of a Kintrinsic CLAUSE or GRANT — the exact
// envelope `charterd` verifies (`spec/contract.md` "Signed-inner-event
// gift-wrap"):
//   inner  31113 (CLAUSE) / 31112 (GRANT)
//                 = a FULLY-SIGNED NIP-01 event by the pinned guardian
//   seal   13     = guardian-authored, nip44(inner) to the device
//   wrap   1059   = ephemeral-authored, nip44(seal) to the device, p-tagged
// The device re-derives the inner id, verifies the inner schnorr sig, and
// checks author == pinned guardian before caching (a CLAUSE) or acting (a
// GRANT). So this MUST produce a signed inner (not a bare rumor) — that's the
// load-bearing bit.

import {
  finalizeEvent,
  generateSecretKey,
  nip44,
  verifyEvent,
  type EventTemplate,
  type NostrEvent,
} from "nostr-tools";
import type { ClausePayload, GrantPayload } from "./types";
import type { UsageSyncPayload } from "./usageSync";

export const CHARTER_DEVICE_GRANT = 31112;
export const CHARTER_DEVICE_CLAUSE = 31113;
export const CHARTER_DEVICE_USAGE_SYNC = 31115;
export const CHARTER_DEVICE_RELEASE = 31116;
export const CHARTER_DEVICE_PAIR_OFFER = 31117;
export const SEAL = 13;
export const GIFT_WRAP = 1059;
export const MARKER: [string, string] = ["t", "charter-device"];

/**
 * The two guardian-key operations the gift-wrap needs. In production these
 * delegate to the Signet bunker over NIP-46 (`sign_event` / `nip44_encrypt`);
 * in tests a local key implements them. The guardian's private key never leaves
 * the signer — this interface is the only contact surface.
 */
export interface GuardianOps {
  /** The pinned guardian pubkey (hex). */
  pubkey: string;
  /** NIP-46 `sign_event`: sign an event template with the guardian key. */
  signEvent(template: EventTemplate): Promise<NostrEvent>;
  /** NIP-46 `nip44_encrypt`: encrypt `plaintext` from the guardian to `recipientPubkey`. */
  nip44Encrypt(recipientPubkey: string, plaintext: string): Promise<string>;
}

/** Sign the inner event of `kind` and gift-wrap it to `devicePubkey` — the
 *  shared CLAUSE/GRANT spread (both are guardian-signed inners). */
async function giftWrapSignedInner(
  kind: number,
  content: string,
  devicePubkey: string,
  guardian: GuardianOps,
  createdAt: number,
): Promise<NostrEvent> {
  // 1) Inner event — signed by the guardian (the load-bearing signature).
  const innerTemplate: EventTemplate = {
    kind,
    created_at: createdAt,
    tags: [MARKER],
    content,
  };
  const inner = await guardian.signEvent(innerTemplate);

  // 2) Seal (13) — guardian-authored, nip44(inner) → device.
  const sealTemplate: EventTemplate = {
    kind: SEAL,
    created_at: createdAt,
    tags: [],
    content: await guardian.nip44Encrypt(devicePubkey, JSON.stringify(inner)),
  };
  const seal = await guardian.signEvent(sealTemplate);

  // 3) Gift wrap (1059) — ephemeral-authored, nip44(seal) → device, p-tagged.
  const ephemeral = generateSecretKey();
  const wrapContent = nip44.encrypt(
    JSON.stringify(seal),
    nip44.getConversationKey(ephemeral, devicePubkey),
  );
  return finalizeEvent(
    {
      kind: GIFT_WRAP,
      created_at: createdAt,
      tags: [["p", devicePubkey]],
      content: wrapContent,
    },
    ephemeral,
  );
}

/**
 * Build the inner CLAUSE event (31113) and gift-wrap it to `devicePubkey`.
 * Returns the outer 1059 ready to publish to the device's relay(s).
 */
export async function giftWrapClause(
  payload: ClausePayload,
  devicePubkey: string,
  guardian: GuardianOps,
  createdAt: number,
): Promise<NostrEvent> {
  return giftWrapSignedInner(
    CHARTER_DEVICE_CLAUSE,
    JSON.stringify(payload),
    devicePubkey,
    guardian,
    createdAt,
  );
}

/**
 * Build the inner GRANT event (31112) and gift-wrap it to the MACHINE that
 * brokered the request. Returns the outer 1059 ready to publish. The device
 * verifies the inner guardian signature + the reqId/nonce echo before enacting.
 */
export async function giftWrapGrant(
  payload: GrantPayload,
  machinePubkey: string,
  guardian: GuardianOps,
  createdAt: number,
): Promise<NostrEvent> {
  return giftWrapSignedInner(
    CHARTER_DEVICE_GRANT,
    JSON.stringify(payload),
    machinePubkey,
    guardian,
    createdAt,
  );
}

/** The RELEASE payload — a parent-gated unpair, targeting one machine. */
export interface ReleasePayload {
  v: 1;
  machine: string;
  issuedAt: number;
}

/**
 * Build the inner RELEASE event (31116) and gift-wrap it to the MACHINE being
 * released. The device verifies the guardian's inner signature + that it
 * targets this machine, then drops its pairing and all enforcement. Only the
 * guardian key can produce it, so a child can't unpair their own device.
 */
export async function giftWrapRelease(
  machinePubkey: string,
  guardian: GuardianOps,
  createdAt: number,
): Promise<NostrEvent> {
  const payload: ReleasePayload = {
    v: 1,
    machine: machinePubkey,
    issuedAt: createdAt,
  };
  return giftWrapSignedInner(
    CHARTER_DEVICE_RELEASE,
    JSON.stringify(payload),
    machinePubkey,
    guardian,
    createdAt,
  );
}

/**
 * Offer to be pinned by an unpaired ward whose pairing QR this phone just
 * scanned (kind 31117).
 *
 * The payload names NO guardian pubkey: the ward pins whoever actually sealed
 * the wrap, so this can only ever nominate us. What it does carry is the
 * one-time `token` from the ward's screen — proof we were standing in front of
 * it, which is the same trust basis as typing on the machine itself.
 */
export async function giftWrapPairOffer(
  machinePubkey: string,
  token: string,
  relays: string[],
  guardian: GuardianOps,
  createdAt: number,
): Promise<NostrEvent> {
  const payload = { token, relays, ts: createdAt };
  return giftWrapSignedInner(
    CHARTER_DEVICE_PAIR_OFFER,
    JSON.stringify(payload),
    machinePubkey,
    guardian,
    createdAt,
  );
}

/**
 * Build the inner USAGE_SYNC event (31115) and gift-wrap it to the receiving
 * MACHINE — the guardian-signed consolidated cross-device usage view the
 * warden pools its budget against (B3). Same envelope as a CLAUSE; the warden
 * verifies the pinned guardian's inner signature + monotonic `ts`.
 */
export async function giftWrapUsageSync(
  payload: UsageSyncPayload,
  machinePubkey: string,
  guardian: GuardianOps,
  createdAt: number,
): Promise<NostrEvent> {
  return giftWrapSignedInner(
    CHARTER_DEVICE_USAGE_SYNC,
    JSON.stringify(payload),
    machinePubkey,
    guardian,
    createdAt,
  );
}

/** Decrypt wrap → seal → inner with the recipient's key — what `charterd` does
 *  on receipt. Throws if `recipientSk` can't decrypt (wrong recipient / tampered). */
function unwrapSignedInner(
  wrap: NostrEvent,
  recipientSk: Uint8Array,
): { seal: NostrEvent; inner: NostrEvent } {
  const seal: NostrEvent = JSON.parse(
    nip44.decrypt(wrap.content, nip44.getConversationKey(recipientSk, wrap.pubkey)),
  );
  const inner: NostrEvent = JSON.parse(
    nip44.decrypt(seal.content, nip44.getConversationKey(recipientSk, seal.pubkey)),
  );
  return { seal, inner };
}

export interface UnwrappedClause {
  payload: ClausePayload;
  inner: NostrEvent;
  /** True iff the inner event's sig verifies AND its author == the pinned guardian. */
  valid: boolean;
}

/**
 * Reverse of {@link giftWrapClause} — what `charterd` does on receipt. Used to
 * verify our own output (and in tests). Throws if the wrap can't be decrypted
 * by `recipientSk` (wrong recipient / tampered). `valid` is false when the
 * inner author isn't the `pinnedGuardian` or the inner sig fails.
 */
export function unwrapClause(
  wrap: NostrEvent,
  recipientSk: Uint8Array,
  pinnedGuardian: string,
): UnwrappedClause {
  const { seal, inner } = unwrapSignedInner(wrap, recipientSk);
  const payload = JSON.parse(inner.content) as ClausePayload;
  const valid =
    inner.kind === CHARTER_DEVICE_CLAUSE &&
    inner.pubkey === pinnedGuardian &&
    seal.kind === SEAL &&
    verifyEvent(inner);
  return { payload, inner, valid };
}

export interface UnwrappedGrant {
  payload: GrantPayload;
  inner: NostrEvent;
  /** True iff the inner event's sig verifies AND its author == the pinned guardian. */
  valid: boolean;
}

/**
 * Reverse of {@link giftWrapGrant} — what `charterd` does before enacting.
 * Same contract as {@link unwrapClause}: throws on a wrap `recipientSk` can't
 * open; `valid` is false on a wrong author / bad inner sig.
 */
export function unwrapGrant(
  wrap: NostrEvent,
  recipientSk: Uint8Array,
  pinnedGuardian: string,
): UnwrappedGrant {
  const { seal, inner } = unwrapSignedInner(wrap, recipientSk);
  const payload = JSON.parse(inner.content) as GrantPayload;
  const valid =
    inner.kind === CHARTER_DEVICE_GRANT &&
    inner.pubkey === pinnedGuardian &&
    seal.kind === SEAL &&
    verifyEvent(inner);
  return { payload, inner, valid };
}
