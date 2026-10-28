// Batch 1 re-review N1 (JSON nesting limit) and N3 (previous relay lists cap).
import { describe, test, expect } from 'vitest'
import {
  applyInbound,
  applyPresenceConfirm,
  registerPresenceToken,
  applyRelays,
  parseStatement,
  parseStrictJson,
  tryParseStrictJson,
  projectState,
  CHARTER_GUARDIAN_STATEMENT,
  MAX_JSON_DEPTH,
  PREVIOUS_RELAYS_MAX,
  type GuardianState,
  type StepResult,
} from '../../src/guardians/index.js'
import { R1, R2, W1, W2, T0, DAY, TOKEN2, approval, coGuardian, confirm, signRaw, pairedR1W1, type Key } from './helpers.js'

const dW = ['d', `kintrinsic/device/${W1.pk}`]
const pW = ['p', W1.pk]
const M = ['t', 'charter-device']
const raw = (c: string) => signRaw(R1, { kind: CHARTER_GUARDIAN_STATEMENT, tags: [dW, pW, M], content: c })
/** `{"v":1,"issuedAt":..,"x":[[...]]}` with the given total depth (top-level object is 1). */
const nested = (depth: number) => `{"v":1,"issuedAt":${T0},"x":${'['.repeat(depth - 1)}${']'.repeat(depth - 1)}}`

describe('N1: JSON nesting limit', () => {
  test('the limit is 16', () => expect(MAX_JSON_DEPTH).toBe(16))
  test('depth 16 parses, depth 17 is content_not_json', () => {
    expect(parseStatement(raw(nested(16))).ok).toBe(true)
    expect(parseStatement(raw(nested(17)))).toEqual({ ok: false, reason: 'content_not_json' })
  })
  test('objects count the same as arrays', () => {
    const objs = (n: number) => '{"a":'.repeat(n) + '1' + '}'.repeat(n)
    expect(tryParseStrictJson(objs(16)).ok).toBe(true)
    expect(tryParseStrictJson(objs(17)).ok).toBe(false)
    expect(tryParseStrictJson('['.repeat(16) + ']'.repeat(16)).ok).toBe(true)
    expect(tryParseStrictJson('['.repeat(17) + ']'.repeat(17)).ok).toBe(false)
  })
  test('siblings do not accumulate depth', () => {
    const a = '['.repeat(15) + ']'.repeat(15)
    expect(tryParseStrictJson(`[${Array(50).fill(a).join(',')}]`).ok).toBe(true) // 1 + 15 = 16 deep
  })
  test('very deep input is refused, not a crash', () => {
    expect(tryParseStrictJson('['.repeat(100_000)).ok).toBe(false)
    expect(() => parseStrictJson('['.repeat(100_000))).toThrow()
  })
  test('a presence confirmation carrying an inline approval still parses', () => {
    const ap = approval(R1, W1, { issuedAt: T0 })
    const ev = confirm(W1, 'ab'.repeat(32), TOKEN2, T0, ap)
    expect(tryParseStrictJson(ev.content).ok).toBe(true)
  })
})

describe('N3: previous relay lists are capped', () => {
  const ok = (r: StepResult): GuardianState => {
    expect(r.outcome.result, r.outcome.reason).toBe('accepted')
    return r.state
  }
  function twoRoots(): GuardianState {
    let s = pairedR1W1()
    const cg = coGuardian(R1, R2, { issuedAt: T0 + 5 })
    s = ok(applyInbound(s, { sealAuthor: W1.pk, inner: cg }, T0 + 10))
    s = ok(registerPresenceToken(s, TOKEN2, `kintrinsic/guardian/${R2.pk}`, T0 + 20))
    return ok(applyPresenceConfirm(s, { sealAuthor: W2.pk, inner: confirm(W2, cg.id, TOKEN2, T0 + 30, approval(R2, W2, { issuedAt: T0 + 15 })) }, T0 + 30))
  }
  const rel = (s: GuardianState, a: Key, ts: number) =>
    ok(applyRelays(s, { author: a.pk, sealAuthor: a.pk, channel: 'wrapped', ts, relays: [`wss://n${ts - T0 - 100}.example`] }, ts))

  test('the cap is 8', () => expect(PREVIOUS_RELAYS_MAX).toBe(8))
  test('a 9th previous list drops the oldest', () => {
    let s = twoRoots()
    // 8 changes: 8 previous lists (the pairing list, 2 URLs, + 7 changes); all kept.
    for (let n = 1; n <= 8; n++) s = rel(s, W1, T0 + 100 + n)
    let sr = projectState(s, T0 + 108).statusRelays
    expect(sr).toHaveLength(1 + 8 + 1)
    expect(sr).toContain('wss://a.example')
    // A 9th change adds a 9th previous list: the pairing list is dropped.
    s = rel(s, W1, T0 + 109)
    sr = projectState(s, T0 + 109).statusRelays
    expect(sr).toHaveLength(1 + 8)
    expect(sr).not.toContain('wss://a.example')
    expect(sr).not.toContain('wss://b.example')
    expect(sr[0]).toBe('wss://n9.example')
    expect(sr).toContain('wss://n1.example')
    // And a 10th drops the next oldest (the list the first change set).
    s = rel(s, W1, T0 + 110)
    sr = projectState(s, T0 + 110).statusRelays
    expect(sr).toHaveLength(1 + 8)
    expect(sr).not.toContain('wss://n1.example')
    expect(sr).toContain('wss://n2.example')
  })
  test('the cap applies per change, not per day: expiry still applies to what is kept', () => {
    let s = twoRoots()
    for (let n = 1; n <= 10; n++) s = rel(s, W1, T0 + 100 + n)
    expect(projectState(s, T0 + 110 + 7 * DAY).statusRelays).toEqual(['wss://n10.example'])
  })
})
