import { generateSecretKey, getPublicKey, finalizeEvent, type NostrEvent } from 'nostr-tools/pure'
import { nip44 } from 'nostr-tools'
import { bytesToHex, hexToBytes } from '@noble/hashes/utils.js'
import type { ChartedClause } from '../../src/schedule/schedule.js'

export interface KeypairHex {
  privkey: string
  pubkey: string
}

export function generateKeypair(): KeypairHex {
  const sk = generateSecretKey()
  return { privkey: bytesToHex(sk), pubkey: getPublicKey(sk) }
}

/**
 * Build a signed kind-31000 Charter clause event addressed to `appPubkey`,
 * with the payload NIP-44 encrypted by the parent (author).
 */
export function makeClauseEvent(args: {
  parent: KeypairHex
  appPubkey: string
  subjectPubkey: string
  payload: ChartedClause
  createdAt?: number
}): NostrEvent {
  const { parent, appPubkey, subjectPubkey, payload } = args
  const conv = nip44.v2.utils.getConversationKey(hexToBytes(parent.privkey), appPubkey)
  const content = nip44.v2.encrypt(JSON.stringify(payload), conv)
  const template = {
    kind: 31000,
    created_at: args.createdAt ?? Math.floor(Date.now() / 1000),
    content,
    tags: [
      ['t', 'charter-clause'],
      ['d', `${subjectPubkey}:${appPubkey}`],
      ['p', appPubkey],
      ['consumer', appPubkey],
    ],
  }
  return finalizeEvent(template, hexToBytes(parent.privkey))
}

export function baseSchedulePayload(overrides: Partial<ChartedClause> = {}): ChartedClause {
  return {
    schemaVersion: 1,
    kind: 'schedule',
    mechanism: 'static-data',
    windows: [],
    timezone: 'UTC',
    endDate: null,
    revoked: false,
    ...overrides,
  }
}
