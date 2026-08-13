import 'fake-indexeddb/auto'
import { describe, test, expect, beforeEach } from 'vitest'
import type { NostrEvent } from 'nostr-tools/pure'
import { createCharterEvaluator } from '../../src/evaluator/evaluator.js'
import { generateKeypair, makeClauseEvent, baseSchedulePayload } from './fixtures.js'

class FakeWebSocket {
  static instances: FakeWebSocket[] = []
  static byUrl: Record<string, FakeWebSocket> = {}
  readonly url: string
  private listeners: Record<string, Array<(evt: unknown) => void>> = {}
  sent: string[] = []
  closed = false

  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
    FakeWebSocket.byUrl[url] = this
  }

  static reset(): void {
    FakeWebSocket.instances = []
    FakeWebSocket.byUrl = {}
  }

  addEventListener(name: string, cb: (evt: unknown) => void): void {
    if (!this.listeners[name]) this.listeners[name] = []
    this.listeners[name].push(cb)
  }

  send(payload: string): void {
    this.sent.push(payload)
  }

  close(): void {
    this.closed = true
    this.fire('close', {})
  }

  fire(name: string, evt: unknown): void {
    for (const cb of this.listeners[name] ?? []) cb(evt)
  }

  fireOpen(): void {
    this.fire('open', {})
  }

  deliverEvent(subId: string, event: NostrEvent): void {
    this.fire('message', { data: JSON.stringify(['EVENT', subId, event]) })
  }

  deliverEose(subId: string): void {
    this.fire('message', { data: JSON.stringify(['EOSE', subId]) })
  }
}

const factory = (url: string) => new FakeWebSocket(url) as unknown as WebSocket

function parseReq(payload: string): { subId: string } {
  const arr = JSON.parse(payload) as unknown[]
  return { subId: arr[1] as string }
}

let dbCounter = 0
beforeEach(() => {
  dbCounter++
  FakeWebSocket.reset()
})
function uniqueDbName(): string {
  return `charter_evaluator_test_${dbCounter}`
}

describe('createCharterEvaluator — pre-init', () => {
  test('check() before init returns no_pairing (allow)', () => {
    const app = generateKeypair()
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
    })
    const subject = generateKeypair().pubkey
    expect(ev.check(subject)).toEqual({ allow: true, reason: 'no_pairing' })
  })

  test('check() with malformed pubkey returns no_pairing', () => {
    const app = generateKeypair()
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
    })
    expect(ev.check('not-hex')).toEqual({ allow: true, reason: 'no_pairing' })
  })
})

describe('createCharterEvaluator.init', () => {
  test('returns no-pairing flags when session is missing required fields', async () => {
    const app = generateKeypair()
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
    })
    const result = await ev.init({
      charterAuthors: [],
      charterRelays: [],
      depCanonicalPubkey: null,
      subjectPubkey: '',
    })
    expect(result).toEqual({ relayConnected: false, hasInitialClause: false })
    expect(FakeWebSocket.instances).toHaveLength(0)
  })

  test('opens connections to every charter relay in parallel', async () => {
    const app = generateKeypair()
    const parent = generateKeypair()
    const subject = generateKeypair().pubkey
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
      coldStartTimeoutMs: 100,
    })

    const promise = ev.init({
      charterAuthors: [parent.pubkey],
      charterRelays: ['wss://a', 'wss://b'],
      depCanonicalPubkey: null,
      subjectPubkey: subject,
    })

    await Promise.resolve()
    expect(FakeWebSocket.byUrl['wss://a']).toBeDefined()
    expect(FakeWebSocket.byUrl['wss://b']).toBeDefined()

    FakeWebSocket.byUrl['wss://a']!.fireOpen()
    FakeWebSocket.byUrl['wss://b']!.fireOpen()
    // No events delivered; timeout will resolve init.
    const result = await promise
    expect(result.relayConnected).toBe(true)
  })
})

describe('createCharterEvaluator.check — with delivered clauses', () => {
  test('delivered always-allow clause grants check()', async () => {
    const app = generateKeypair()
    const parent = generateKeypair()
    const subject = generateKeypair().pubkey
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
      coldStartTimeoutMs: 1000,
    })

    const clauseEvent = makeClauseEvent({
      parent,
      appPubkey: app.pubkey,
      subjectPubkey: subject,
      payload: baseSchedulePayload({ windows: [] }),
    })

    const initP = ev.init({
      charterAuthors: [parent.pubkey],
      charterRelays: ['wss://a'],
      depCanonicalPubkey: null,
      subjectPubkey: subject,
    })

    await Promise.resolve()
    const ws = FakeWebSocket.byUrl['wss://a']!
    ws.fireOpen()
    const { subId } = parseReq(ws.sent[0]!)
    ws.deliverEvent(subId, clauseEvent)
    ws.deliverEose(subId)

    await initP
    const result = ev.check(subject)
    expect(result.allow).toBe(true)
    expect(result.reason).toBe('always_allow')
  })

  test('delivered revoked clause denies check()', async () => {
    const app = generateKeypair()
    const parent = generateKeypair()
    const subject = generateKeypair().pubkey
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
      coldStartTimeoutMs: 1000,
    })

    const clauseEvent = makeClauseEvent({
      parent,
      appPubkey: app.pubkey,
      subjectPubkey: subject,
      payload: baseSchedulePayload({ revoked: true }),
    })

    const initP = ev.init({
      charterAuthors: [parent.pubkey],
      charterRelays: ['wss://a'],
      depCanonicalPubkey: null,
      subjectPubkey: subject,
    })

    await Promise.resolve()
    const ws = FakeWebSocket.byUrl['wss://a']!
    ws.fireOpen()
    const { subId } = parseReq(ws.sent[0]!)
    ws.deliverEvent(subId, clauseEvent)
    ws.deliverEose(subId)

    await initP
    const result = ev.check(subject)
    expect(result.allow).toBe(false)
    expect(result.reason).toBe('clause_revoked')
  })

  test('clause from non-authorized author is dropped', async () => {
    const app = generateKeypair()
    const authorisedParent = generateKeypair()
    const rogueParent = generateKeypair()
    const subject = generateKeypair().pubkey
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
      coldStartTimeoutMs: 1000,
    })

    // Rogue author tries to deny.
    const rogueClause = makeClauseEvent({
      parent: rogueParent,
      appPubkey: app.pubkey,
      subjectPubkey: subject,
      payload: baseSchedulePayload({ revoked: true }),
    })

    const initP = ev.init({
      charterAuthors: [authorisedParent.pubkey],
      charterRelays: ['wss://a'],
      depCanonicalPubkey: null,
      subjectPubkey: subject,
    })

    await Promise.resolve()
    const ws = FakeWebSocket.byUrl['wss://a']!
    ws.fireOpen()
    const { subId } = parseReq(ws.sent[0]!)
    ws.deliverEvent(subId, rogueClause)
    ws.deliverEose(subId)

    await initP
    // No clause from the authorised parent → no_clause (allow).
    expect(ev.check(subject)).toEqual({ allow: true, reason: 'no_clause' })
  })

  test('relay TCP failure (zero connects) gives relay_unavailable on check', async () => {
    const app = generateKeypair()
    const parent = generateKeypair()
    const subject = generateKeypair().pubkey
    const throwingFactory = () => { throw new Error('relay unreachable') }

    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: throwingFactory,
      coldStartTimeoutMs: 50,
    })

    await ev.init({
      charterAuthors: [parent.pubkey],
      charterRelays: ['wss://broken'],
      depCanonicalPubkey: null,
      subjectPubkey: subject,
    })

    expect(ev.check(subject)).toEqual({ allow: false, reason: 'relay_unavailable' })
  })

  test('TCP-connected but no clause delivered within timeout → no_clause (allow)', async () => {
    const app = generateKeypair()
    const parent = generateKeypair()
    const subject = generateKeypair().pubkey
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
      coldStartTimeoutMs: 50,
    })

    const initP = ev.init({
      charterAuthors: [parent.pubkey],
      charterRelays: ['wss://a'],
      depCanonicalPubkey: null,
      subjectPubkey: subject,
    })

    await Promise.resolve()
    FakeWebSocket.byUrl['wss://a']!.fireOpen()
    // No events delivered before timeout.

    await initP
    expect(ev.check(subject)).toEqual({ allow: true, reason: 'no_clause' })
  })
})

describe('createCharterEvaluator.shutdown', () => {
  test('closes open WebSockets and resets check() to no_pairing', async () => {
    const app = generateKeypair()
    const parent = generateKeypair()
    const subject = generateKeypair().pubkey
    const ev = createCharterEvaluator({
      appKeypair: app,
      dbName: uniqueDbName(),
      webSocketFactory: factory,
      coldStartTimeoutMs: 1000,
    })

    const initP = ev.init({
      charterAuthors: [parent.pubkey],
      charterRelays: ['wss://a'],
      depCanonicalPubkey: null,
      subjectPubkey: subject,
    })
    await Promise.resolve()
    const ws = FakeWebSocket.byUrl['wss://a']!
    ws.fireOpen()
    const { subId } = parseReq(ws.sent[0]!)
    ws.deliverEose(subId)
    await initP

    await ev.shutdown()
    expect(ws.closed).toBe(true)
    expect(ev.check(subject)).toEqual({ allow: true, reason: 'no_pairing' })
  })
})
