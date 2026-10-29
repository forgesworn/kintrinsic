import { finalizeEvent, getPublicKey } from 'nostr-tools/pure'
import {
  approvalTemplate,
  cancelTemplate,
  coGuardianTemplate,
  objectionTemplate,
  pairFromOffer,
  pairOfferTemplate,
  presenceConfirmTemplate,
  rootMoveTemplate,
  rootRemovalTemplate,
  withdrawalTemplate,
  type EventTemplate,
  type GuardianState,
  type SignedEvent,
  type StatementOptions,
} from '../../src/guardians/index.js'

export interface Key {
  sk: Uint8Array
  pk: string
}

export function key(n: number, page = 1): Key {
  const sk = new Uint8Array(32)
  sk[30] = page
  sk[31] = n
  return { sk, pk: getPublicKey(sk) }
}

export function sign(k: Key, t: EventTemplate): SignedEvent {
  return finalizeEvent({ ...t, tags: t.tags.map((x) => [...x]) }, k.sk) as SignedEvent
}

/** Sign an arbitrary template (for malformed-statement cases). */
export function signRaw(k: Key, t: { kind: number; tags: string[][]; content: string; created_at?: number }): SignedEvent {
  return finalizeEvent({ created_at: 1, ...t }, k.sk) as SignedEvent
}

export const R1 = key(1)
export const R2 = key(2)
export const R3 = key(3)
export const R4 = key(4)
export const R5 = key(5)
export const W1 = key(11)
export const W1b = key(12)
export const W2 = key(21)
export const W3 = key(31)
export const W4 = key(41)
export const X = key(99)

export const T0 = 1_800_000_000
export const DAY = 86_400
export const TOKEN = 'ab'.repeat(16)
export const TOKEN2 = 'cd'.repeat(16)

export const approval = (r: Key, w: Key, o: StatementOptions) => sign(r, approvalTemplate(w.pk, o))
export const withdrawal = (r: Key, w: Key, o: StatementOptions) => sign(r, withdrawalTemplate(w.pk, o))
export const coGuardian = (r: Key, r2: Key, o: StatementOptions) => sign(r, coGuardianTemplate(r2.pk, o))
export const removal = (by: Key, r2: Key, o: StatementOptions) => sign(by, rootRemovalTemplate(r2.pk, o))
export const move = (r: Key, r2: Key, o: StatementOptions) => sign(r, rootMoveTemplate(r2.pk, o))
export const cancel = (by: Key, pendingD: string, o: StatementOptions) => sign(by, cancelTemplate(pendingD, o))
export const objection = (by: Key, removalD: string, remover: Key, o: StatementOptions) =>
  sign(by, objectionTemplate(removalD, remover.pk, o))
export const confirm = (w: Key, statement: string, token: string, now: number, ap?: SignedEvent) =>
  sign(w, presenceConfirmTemplate({ token, statement, approval: ap, createdAt: now }))

/** A ward paired at T0 by W1 carrying R1's approval. */
export function pairedR1W1(): GuardianState {
  const ap = approval(R1, W1, { issuedAt: T0 - 100, label: 'Sam phone' })
  const offer = sign(W1, pairOfferTemplate({ token: TOKEN, relays: ['wss://a.example', 'wss://b.example'], ts: T0, approval: ap }))
  const r = pairFromOffer({ sealAuthor: W1.pk, inner: offer }, { token: TOKEN, mintedAt: T0 - 10 }, T0)
  if (!r.ok) throw new Error(`pair failed: ${r.reason}`)
  return r.state
}

/** A ward paired at T0 by R1 directly (no working key). */
export function pairedR1(): GuardianState {
  const offer = sign(R1, pairOfferTemplate({ token: TOKEN, relays: ['wss://a.example', 'wss://b.example'], ts: T0 }))
  const r = pairFromOffer({ sealAuthor: R1.pk, inner: offer }, { token: TOKEN, mintedAt: T0 - 10 }, T0)
  if (!r.ok) throw new Error(`pair failed: ${r.reason}`)
  return r.state
}
