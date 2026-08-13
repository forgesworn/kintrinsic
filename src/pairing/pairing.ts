import { BunkerSigner, parseBunkerInput } from 'nostr-tools/nip46'
import { bytesToHex, hexToBytes, randomBytes } from '@noble/hashes/utils.js'

const HEX_64 = /^[0-9a-f]{64}$/i
const STORE = 'pairings'
const DB_VERSION = 1
const DEFAULT_DB_NAME = 'charter_pairings'

export interface CreatePairingStoreOptions {
  /** IndexedDB database name. Default: `charter_pairings`. Override if you
   * need multiple isolated stores in one origin (rare). */
  dbName?: string
}

export type PairResult =
  | { ok: true; subjectPubkey: string }
  | { ok: false; error: string }

export interface PairingStore {
  pair(bunkerUri: string, subjectPubkey: string): Promise<PairResult>
  has(subjectPubkey: string): Promise<boolean>
  getSigner(subjectPubkey: string): Promise<BunkerSigner>
  unpair(subjectPubkey: string): Promise<void>
}

interface PairingRecord {
  subject_pubkey: string
  bunker_uri: string
  bunker_pointer: {
    pubkey: string
    relays: string[]
    secret: string | null
  }
  local_secret_hex: string
  paired_at: number
  last_used_at: number
}

function validPubkey(pk: unknown): pk is string {
  return typeof pk === 'string' && HEX_64.test(pk)
}

function safeCloseSigner(signer: BunkerSigner | undefined): void {
  if (!signer || typeof signer.close !== 'function') return
  try {
    const p = signer.close()
    if (p && typeof (p as Promise<void>).then === 'function') {
      ;(p as Promise<void>).catch(() => { /* swallow */ })
    }
  } catch { /* swallow */ }
}

/**
 * IDB-backed bunker pairings store. One record per subject pubkey.
 *
 * Per-device by design — pairings are not portable across devices.
 * If the user switches device the parent must re-pair.
 *
 * Generalised from examplegame's `charter-pairing.js` (Phase 1α).
 */
export function createPairingStore(opts: CreatePairingStoreOptions = {}): PairingStore {
  const dbName = opts.dbName ?? DEFAULT_DB_NAME
  const signerCache = new Map<string, BunkerSigner>()

  function openDb(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(dbName, DB_VERSION)
      req.onupgradeneeded = () => {
        const db = req.result
        if (!db.objectStoreNames.contains(STORE)) {
          db.createObjectStore(STORE, { keyPath: 'subject_pubkey' })
        }
      }
      req.onsuccess = () => resolve(req.result)
      req.onerror = () => reject(req.error ?? new Error('open failed'))
    })
  }

  async function readRecord(subject: string): Promise<PairingRecord | null> {
    const db = await openDb()
    try {
      return await new Promise<PairingRecord | null>((resolve, reject) => {
        const tx = db.transaction(STORE, 'readonly')
        const req = tx.objectStore(STORE).get(subject)
        req.onsuccess = () => resolve((req.result as PairingRecord) ?? null)
        req.onerror = () => reject(req.error ?? new Error('get failed'))
      })
    } finally {
      db.close()
    }
  }

  async function touchLastUsed(subject: string): Promise<void> {
    const db = await openDb()
    try {
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction(STORE, 'readwrite')
        tx.oncomplete = () => resolve()
        tx.onerror = () => reject(tx.error ?? new Error('tx failed'))
        const store = tx.objectStore(STORE)
        const req = store.get(subject)
        req.onsuccess = () => {
          const rec = req.result as PairingRecord | undefined
          if (!rec) return
          rec.last_used_at = Date.now()
          store.put(rec)
        }
      })
    } finally {
      db.close()
    }
  }

  async function pair(bunkerUri: string, subjectPubkey: string): Promise<PairResult> {
    if (!validPubkey(subjectPubkey)) {
      return { ok: false, error: 'invalid subject pubkey' }
    }
    if (typeof bunkerUri !== 'string' || bunkerUri.indexOf('bunker://') !== 0) {
      return { ok: false, error: 'expected a bunker:// URI' }
    }

    let parsed: Awaited<ReturnType<typeof parseBunkerInput>>
    try {
      parsed = await parseBunkerInput(bunkerUri)
    } catch (e) {
      return { ok: false, error: `bunker URI parse failed: ${(e as Error).message}` }
    }
    if (!parsed || !parsed.pubkey || !Array.isArray(parsed.relays) || parsed.relays.length === 0) {
      return { ok: false, error: 'bunker URI missing pubkey or relays' }
    }

    const localSecretBytes = randomBytes(32)
    const local_secret_hex = bytesToHex(localSecretBytes)
    const subject = subjectPubkey.toLowerCase()
    const now = Date.now()
    const record: PairingRecord = {
      subject_pubkey: subject,
      bunker_uri: bunkerUri,
      bunker_pointer: {
        pubkey: parsed.pubkey,
        relays: parsed.relays.slice(),
        secret: parsed.secret ?? null,
      },
      local_secret_hex,
      paired_at: now,
      last_used_at: now,
    }

    const db = await openDb()
    try {
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction(STORE, 'readwrite')
        tx.oncomplete = () => resolve()
        tx.onerror = () => reject(tx.error ?? new Error('tx failed'))
        tx.onabort = () => reject(tx.error ?? new Error('tx aborted'))
        tx.objectStore(STORE).put(record)
      })
    } finally {
      db.close()
    }

    // Re-pair invalidates any cached signer — the old pointer might be stale.
    const stale = signerCache.get(subject)
    if (stale) {
      safeCloseSigner(stale)
      signerCache.delete(subject)
    }

    return { ok: true, subjectPubkey: subject }
  }

  async function has(subjectPubkey: string): Promise<boolean> {
    if (!validPubkey(subjectPubkey)) return false
    const rec = await readRecord(subjectPubkey.toLowerCase())
    return !!rec
  }

  async function getSigner(subjectPubkey: string): Promise<BunkerSigner> {
    if (!validPubkey(subjectPubkey)) {
      throw new Error('invalid subject pubkey')
    }
    const subject = subjectPubkey.toLowerCase()
    const cached = signerCache.get(subject)
    if (cached) return cached

    const rec = await readRecord(subject)
    if (!rec) {
      throw new Error(`no pairing for ${subject.slice(0, 8)}…`)
    }

    let signer: BunkerSigner
    try {
      signer = BunkerSigner.fromBunker(
        hexToBytes(rec.local_secret_hex),
        {
          pubkey: rec.bunker_pointer.pubkey,
          relays: rec.bunker_pointer.relays,
          secret: rec.bunker_pointer.secret,
        },
        {},
      )
    } catch (e) {
      throw new Error(`bunker rehydrate failed: ${(e as Error).message}`, { cause: e })
    }

    signerCache.set(subject, signer)
    touchLastUsed(subject).catch(() => { /* swallow */ })
    return signer
  }

  async function unpair(subjectPubkey: string): Promise<void> {
    if (!validPubkey(subjectPubkey)) return
    const subject = subjectPubkey.toLowerCase()
    const cached = signerCache.get(subject)
    if (cached) {
      safeCloseSigner(cached)
      signerCache.delete(subject)
    }
    const db = await openDb()
    try {
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction(STORE, 'readwrite')
        tx.oncomplete = () => resolve()
        tx.onerror = () => reject(tx.error ?? new Error('tx failed'))
        tx.objectStore(STORE).delete(subject)
      })
    } finally {
      db.close()
    }
  }

  return { pair, has, getSigner, unpair }
}
