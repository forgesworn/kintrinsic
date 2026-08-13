import { verifyEvent, type NostrEvent } from 'nostr-tools/pure'
import { nip44 } from 'nostr-tools'
import { hexToBytes } from '@noble/hashes/utils.js'
import {
  evaluateSchedule,
  validateClausePayload,
  type ChartedClause,
  type EvaluateScheduleResult,
} from '../schedule/schedule.js'

const HEX_64 = /^[0-9a-f]{64}$/i
const ATTESTATION_KIND = 31000
const DEFAULT_DB_NAME = 'charter_clause_cache'
const DB_VERSION = 1
const STORE = 'clauses'
const DEFAULT_COLD_START_MS = 5000

export interface ChartedSession {
  charterAuthors: string[]
  charterRelays: string[]
  depCanonicalPubkey: string | null
  subjectPubkey: string
}

export interface CreateEvaluatorOptions {
  appKeypair: {
    pubkey: string
    privkey: string
  }
  dbName?: string
  coldStartTimeoutMs?: number
  webSocketFactory?: (url: string) => WebSocket
}

export type CharterCheckResult =
  | { allow: true; reason?: 'always_allow' | 'no_clause' | 'no_pairing' }
  | { allow: false; reason: 'clause_revoked' | 'clause_expired' | 'schedule_locked' | 'relay_unavailable' }

export interface CharterEvaluator {
  init(session: ChartedSession): Promise<{ relayConnected: boolean; hasInitialClause: boolean }>
  check(subjectPubkey: string, now?: Date): CharterCheckResult
  shutdown(): Promise<void>
}

interface ClauseRecord {
  payload: ChartedClause
  eventId: string
  created_at: number
  receivedAt: number
}

interface RelayHandle {
  url: string
  ws: WebSocket
  subId: string
  closed: boolean
}

function defaultWebSocketFactory(url: string): WebSocket {
  return new (globalThis as { WebSocket: new (u: string) => WebSocket }).WebSocket(url)
}

/**
 * Live Charter clause evaluator. Subscribes to a parent's charter_relays
 * for kind-31000 clause events addressed to the consumer's app pubkey,
 * NIP-44 decrypts, caches the latest clause per (subject, consumer) in
 * IndexedDB, and exposes a synchronous gate the engine calls per session.
 *
 * Generalised from examplegame's `charter-evaluator.js` (Phase 1α rev. 7).
 * Self-report (kind-30471) is intentionally deferred from v0.1 — caller
 * can layer it on top by listening for `check()` results and publishing
 * their own gift-wrapped audit events.
 */
export function createCharterEvaluator(opts: CreateEvaluatorOptions): CharterEvaluator {
  const dbName = opts.dbName ?? DEFAULT_DB_NAME
  const coldStartMs = opts.coldStartTimeoutMs ?? DEFAULT_COLD_START_MS
  const factory = opts.webSocketFactory ?? defaultWebSocketFactory
  const appPubkey = opts.appKeypair.pubkey.toLowerCase()
  const appPrivkeyBytes = hexToBytes(opts.appKeypair.privkey)

  let session: ChartedSession | null = null
  let initialized = false
  let initRelayConnects = 0
  const clauseCache = new Map<string, ClauseRecord>()
  const seenEventIds = new Set<string>()
  const relayHandles: RelayHandle[] = []

  function openDb(): Promise<IDBDatabase> {
    return new Promise((resolve, reject) => {
      const req = indexedDB.open(dbName, DB_VERSION)
      req.onupgradeneeded = () => {
        const db = req.result
        if (!db.objectStoreNames.contains(STORE)) {
          db.createObjectStore(STORE, { keyPath: 'key' })
        }
      }
      req.onsuccess = () => resolve(req.result)
      req.onerror = () => reject(req.error ?? new Error('open failed'))
    })
  }

  async function persistClause(subject: string, consumer: string, record: ClauseRecord): Promise<void> {
    const db = await openDb()
    try {
      await new Promise<void>((resolve, reject) => {
        const tx = db.transaction(STORE, 'readwrite')
        tx.oncomplete = () => resolve()
        tx.onerror = () => reject(tx.error)
        tx.objectStore(STORE).put({
          key: `${subject}:${consumer}`,
          subject_pubkey: subject,
          consumer_pubkey: consumer,
          payload: record.payload,
          eventId: record.eventId,
          created_at: record.created_at,
          cachedAt: Date.now(),
        })
      })
    } finally {
      db.close()
    }
  }

  async function loadCachedClauseFromIdb(subject: string, consumer: string): Promise<{
    payload: ChartedClause
    eventId: string
    created_at: number
    cachedAt: number
  } | null> {
    const db = await openDb()
    try {
      return await new Promise((resolve) => {
        const tx = db.transaction(STORE, 'readonly')
        const req = tx.objectStore(STORE).get(`${subject}:${consumer}`)
        req.onsuccess = () => resolve((req.result as {
          payload: ChartedClause; eventId: string; created_at: number; cachedAt: number
        }) ?? null)
        req.onerror = () => resolve(null)
      })
    } finally {
      db.close()
    }
  }

  function processIncomingEvent(event: NostrEvent): void {
    if (!session) return
    if (!event || typeof event.id !== 'string') return
    if (seenEventIds.has(event.id)) return
    seenEventIds.add(event.id)

    if (!verifyEvent(event)) return
    if (!session.charterAuthors.includes(event.pubkey)) return

    const dTag = (event.tags ?? []).find((t) => Array.isArray(t) && t[0] === 'd')
    if (!dTag || typeof dTag[1] !== 'string') return
    const parts = dTag[1].split(':')
    if (parts.length !== 2) return
    const [subject, consumer] = parts as [string, string]
    if (consumer !== appPubkey) return

    let payload: unknown
    try {
      const conv = nip44.v2.utils.getConversationKey(appPrivkeyBytes, event.pubkey)
      const plaintext = nip44.v2.decrypt(event.content, conv)
      payload = JSON.parse(plaintext)
    } catch {
      return
    }
    if (!validateClausePayload(payload)) return

    const key = `${subject}:${consumer}`
    const existing = clauseCache.get(key)
    if (existing && typeof existing.created_at === 'number' && existing.created_at > event.created_at) {
      return
    }

    const record: ClauseRecord = {
      payload,
      eventId: event.id,
      created_at: event.created_at,
      receivedAt: Date.now(),
    }
    clauseCache.set(key, record)
    persistClause(subject, consumer, record).catch(() => { /* best-effort */ })
  }

  function buildSubjectFilter(): Record<string, unknown> {
    const subject = (session!.depCanonicalPubkey ?? session!.subjectPubkey).toLowerCase()
    return {
      kinds: [ATTESTATION_KIND],
      authors: session!.charterAuthors.slice(),
      '#t': ['charter-clause'],
      '#p': [appPubkey],
      '#d': [`${subject}:${appPubkey}`],
    }
  }

  function connectAndSubscribe(url: string, filter: Record<string, unknown>): Promise<void> {
    return new Promise<void>((resolveConnect) => {
      let ws: WebSocket
      try {
        ws = factory(url)
      } catch {
        resolveConnect()
        return
      }
      const subId = 'ce-' + Math.random().toString(36).slice(2, 10)
      const handle: RelayHandle = { url, ws, subId, closed: false }
      relayHandles.push(handle)

      ws.addEventListener('open', () => {
        initRelayConnects++
        try {
          ws.send(JSON.stringify(['REQ', subId, filter]))
        } catch { /* swallow */ }
        resolveConnect()
      })
      ws.addEventListener('message', (evt: MessageEvent) => {
        let msg: unknown
        try { msg = JSON.parse(typeof evt.data === 'string' ? evt.data : '') } catch { return }
        if (!Array.isArray(msg)) return
        if (msg[0] === 'EVENT' && msg[1] === subId && msg[2] && typeof msg[2] === 'object') {
          processIncomingEvent(msg[2] as NostrEvent)
        }
        // EOSE/CLOSED don't close the subscription — clauses can land later.
      })
      ws.addEventListener('error', () => {
        // Resolve the connect promise; init treats the relay as failed-to-connect.
        if (!handle.closed) resolveConnect()
      })
      ws.addEventListener('close', () => {
        handle.closed = true
        if (!initialized) resolveConnect()
      })
    })
  }

  async function init(input: ChartedSession): Promise<{ relayConnected: boolean; hasInitialClause: boolean }> {
    if (initialized) await shutdown()
    seenEventIds.clear()
    clauseCache.clear()

    if (
      !input ||
      !Array.isArray(input.charterAuthors) || input.charterAuthors.length === 0 ||
      !Array.isArray(input.charterRelays) || input.charterRelays.length === 0 ||
      typeof input.subjectPubkey !== 'string' || !HEX_64.test(input.subjectPubkey)
    ) {
      session = null
      initialized = true
      return { relayConnected: false, hasInitialClause: false }
    }

    session = {
      charterAuthors: input.charterAuthors.slice(),
      charterRelays: input.charterRelays.slice(),
      depCanonicalPubkey: input.depCanonicalPubkey ? input.depCanonicalPubkey.toLowerCase() : null,
      subjectPubkey: input.subjectPubkey.toLowerCase(),
    }
    initRelayConnects = 0

    const subjectKey = session.depCanonicalPubkey ?? session.subjectPubkey

    // Start relay subscriptions FIRST so the cold-start clock measures the
    // network round-trip, not the IDB read. IDB cache load runs in parallel
    // and seeds the cache for the synchronous gate.
    const filter = buildSubjectFilter()
    const subPromises = session.charterRelays.map((url) => connectAndSubscribe(url, filter))

    const cachedP = loadCachedClauseFromIdb(subjectKey, appPubkey).then((cached) => {
      if (cached && cached.payload && validateClausePayload(cached.payload)) {
        clauseCache.set(`${subjectKey}:${appPubkey}`, {
          payload: cached.payload,
          eventId: cached.eventId,
          created_at: cached.created_at,
          receivedAt: cached.cachedAt,
        })
        seenEventIds.add(cached.eventId)
      }
    })

    await Promise.race([
      Promise.all([...subPromises, cachedP]),
      new Promise<void>((r) => setTimeout(r, coldStartMs)),
    ])
    // Give the message loop a moment to deliver buffered events before reporting.
    await new Promise<void>((r) => setTimeout(r, 0))

    initialized = true
    return {
      relayConnected: initRelayConnects > 0,
      hasInitialClause: clauseCache.has(`${subjectKey}:${appPubkey}`),
    }
  }

  function check(subjectPubkeyHex: string, now?: Date): CharterCheckResult {
    if (!initialized || !session) {
      return { allow: true, reason: 'no_pairing' }
    }
    if (typeof subjectPubkeyHex !== 'string' || !HEX_64.test(subjectPubkeyHex)) {
      return { allow: true, reason: 'no_pairing' }
    }
    const subject = subjectPubkeyHex.toLowerCase()
    const sessionSubject = session.depCanonicalPubkey ?? session.subjectPubkey
    const lookupKeys = sessionSubject === subject
      ? [`${subject}:${appPubkey}`]
      : [`${sessionSubject}:${appPubkey}`, `${subject}:${appPubkey}`]

    let cached: ClauseRecord | undefined
    for (const k of lookupKeys) {
      const c = clauseCache.get(k)
      if (c) { cached = c; break }
    }

    if (!cached) {
      if (initRelayConnects === 0 && session.charterRelays.length > 0) {
        return { allow: false, reason: 'relay_unavailable' }
      }
      return { allow: true, reason: 'no_clause' }
    }

    const result = evaluateSchedule(cached.payload, now ?? new Date())
    if (result.allow) {
      return { allow: true, reason: result.reason as 'always_allow' | 'no_clause' | undefined }
    }
    return { allow: false, reason: result.reason as 'clause_revoked' | 'clause_expired' | 'schedule_locked' }
  }

  async function shutdown(): Promise<void> {
    for (const h of relayHandles) {
      try { h.ws.close() } catch { /* swallow */ }
      h.closed = true
    }
    relayHandles.length = 0
    session = null
    initialized = false
    initRelayConnects = 0
    clauseCache.clear()
    seenEventIds.clear()
  }

  return { init, check, shutdown }
}

// Re-export for downstream type consumers
export type { EvaluateScheduleResult }
