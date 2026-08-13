import 'fake-indexeddb/auto'
import { describe, test, expect, beforeEach } from 'vitest'
import { generateSecretKey, getPublicKey, finalizeEvent, type EventTemplate } from 'nostr-tools/pure'
import { bytesToHex } from '@noble/hashes/utils.js'
import { createPairingStore } from '../../src/pairing/pairing.js'

/**
 * Build a sample bunker:// URI valid enough for `parseBunkerInput` to accept.
 * We do NOT spin up a real bunker — we only need a syntactically valid URI
 * so the pair() flow exercises the storage path.
 */
function sampleBunkerUri(): { uri: string; bunkerPubkey: string } {
  const sk = generateSecretKey()
  const pk = getPublicKey(sk)
  return {
    uri: `bunker://${pk}?relay=wss%3A%2F%2Frelay.example.com&secret=abc123`,
    bunkerPubkey: pk,
  }
}

function sampleSubjectPubkey(): string {
  const sk = generateSecretKey()
  return getPublicKey(sk)
}

// Each test gets a fresh IDB instance to avoid cross-test pollution.
let dbCounter = 0
beforeEach(() => {
  dbCounter++
})
function uniqueDbName(): string {
  return `charter_pairings_test_${dbCounter}`
}

describe('createPairingStore.pair', () => {
  test('returns ok with normalised subject when given a valid bunker URI', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const { uri } = sampleBunkerUri()
    const subject = sampleSubjectPubkey()

    const result = await store.pair(uri, subject)
    expect(result.ok).toBe(true)
    if (!result.ok) throw new Error('unreachable')
    expect(result.subjectPubkey).toBe(subject.toLowerCase())
  })

  test('rejects an invalid subject pubkey', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const { uri } = sampleBunkerUri()
    const result = await store.pair(uri, 'not-hex')
    expect(result.ok).toBe(false)
    if (result.ok) throw new Error('unreachable')
    expect(result.error.toLowerCase()).toMatch(/subject/)
  })

  test('rejects a non-bunker URI', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const subject = sampleSubjectPubkey()
    const result = await store.pair('https://example.com/login', subject)
    expect(result.ok).toBe(false)
    if (result.ok) throw new Error('unreachable')
    expect(result.error.toLowerCase()).toMatch(/bunker/)
  })

  test('rejects a malformed bunker URI (missing relay)', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const subject = sampleSubjectPubkey()
    // Pubkey present but no relay query param → parseBunkerInput either
    // returns null or yields an empty relays array → store rejects.
    const sk = generateSecretKey()
    const pk = getPublicKey(sk)
    const result = await store.pair(`bunker://${pk}`, subject)
    expect(result.ok).toBe(false)
  })
})

describe('createPairingStore.has + unpair', () => {
  test('has() is false before pair, true after', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const subject = sampleSubjectPubkey()
    expect(await store.has(subject)).toBe(false)

    const { uri } = sampleBunkerUri()
    await store.pair(uri, subject)
    expect(await store.has(subject)).toBe(true)
  })

  test('has() is false for unknown subject', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    expect(await store.has(sampleSubjectPubkey())).toBe(false)
  })

  test('has() returns false for invalid pubkey input', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    expect(await store.has('not-hex')).toBe(false)
  })

  test('unpair removes the record so has() returns false', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const subject = sampleSubjectPubkey()
    const { uri } = sampleBunkerUri()
    await store.pair(uri, subject)
    expect(await store.has(subject)).toBe(true)

    await store.unpair(subject)
    expect(await store.has(subject)).toBe(false)
  })

  test('unpair is a no-op for unknown subject', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    // Should not throw.
    await store.unpair(sampleSubjectPubkey())
  })
})

describe('createPairingStore.getSigner', () => {
  test('throws when no pairing exists', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const subject = sampleSubjectPubkey()
    await expect(store.getSigner(subject)).rejects.toThrow(/no pairing|not found/i)
  })

  test('throws when given an invalid pubkey', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    await expect(store.getSigner('not-hex')).rejects.toThrow()
  })

  test('returns a BunkerSigner-shaped object when pairing exists', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const subject = sampleSubjectPubkey()
    const { uri } = sampleBunkerUri()
    await store.pair(uri, subject)

    const signer = await store.getSigner(subject)
    // BunkerSigner has a `close` method per nostr-tools/nip46 API.
    expect(typeof signer.close).toBe('function')
    // Tidy up so the test exits cleanly.
    try { await signer.close() } catch { /* ignore */ }
  })

  test('repaired subject invalidates the cached signer', async () => {
    const store = createPairingStore({ dbName: uniqueDbName() })
    const subject = sampleSubjectPubkey()
    const { uri: uri1 } = sampleBunkerUri()
    const { uri: uri2 } = sampleBunkerUri()

    await store.pair(uri1, subject)
    const signer1 = await store.getSigner(subject)
    await store.pair(uri2, subject)
    const signer2 = await store.getSigner(subject)

    expect(signer1).not.toBe(signer2)
    try { await signer1.close() } catch {}
    try { await signer2.close() } catch {}
  })
})

// Suppress unused warnings — these helpers are exported for future use.
void finalizeEvent
void bytesToHex
type _t = EventTemplate
