// Cases from the independent review of batch 1 (H1–H4, M1–M3, L1–L7).
import { describe, test, expect } from 'vitest'
import {
  applyInbound,
  applyPresenceConfirm,
  registerPresenceToken,
  acceptClause,
  applyRelays,
  tick,
  parseStatement,
  projectState,
  isSigner,
  heldClause,
  CHARTER_GUARDIAN_STATEMENT,
  type GuardianState,
  type SignedEvent,
  type StepResult,
} from '../../src/guardians/index.js'
import {
  R1, R2, R3, W1, W1b, W2, W3, T0, DAY, TOKEN, TOKEN2,
  approval, withdrawal, coGuardian, removal, move, cancel, objection, confirm, signRaw,
  pairedR1W1, type Key,
} from './helpers.js'

const H72 = 72 * 3600
const inb = (s: GuardianState, seal: Key, ev: SignedEvent, now: number): StepResult =>
  applyInbound(s, { sealAuthor: seal.pk, inner: ev }, now)
const conf = (s: GuardianState, w: Key, ev: SignedEvent, now: number) => applyPresenceConfirm(s, { sealAuthor: w.pk, inner: ev }, now)
const ok = (r: StepResult, reason?: string): GuardianState => {
  expect(r.outcome.result, r.outcome.reason).toBe('accepted')
  if (reason) expect(r.outcome.reason).toBe(reason)
  return r.state
}
const dropped = (r: StepResult, reason: string) => {
  expect(r.outcome).toMatchObject({ result: 'dropped', reason })
  return r.state
}
const dG = (k: Key) => `kintrinsic/guardian/${k.pk}`
const dU = (k: Key) => `kintrinsic/unguardian/${k.pk}`

function twoRoots(): GuardianState {
  let s = pairedR1W1()
  const cg = coGuardian(R1, R2, { issuedAt: T0 + 5 })
  s = ok(inb(s, W1, cg, T0 + 10))
  s = ok(registerPresenceToken(s, TOKEN2, dG(R2), T0 + 20))
  return ok(conf(s, W2, confirm(W2, cg.id, TOKEN2, T0 + 30, approval(R2, W2, { issuedAt: T0 + 15 })), T0 + 30))
}

describe('H1/L2: a confirmation re-checks R2', () => {
  test('a co-guardian and a move for the same R2: the second is gone once R2 is pinned', () => {
    let s = pairedR1W1()
    const cg = coGuardian(R1, R2, { issuedAt: T0 + 10 })
    const mv = move(R1, R2, { issuedAt: T0 + 11 })
    s = ok(inb(s, W1, cg, T0 + 20))
    s = ok(inb(s, W1, mv, T0 + 20))
    s = ok(registerPresenceToken(s, TOKEN, null, T0 + 30))
    s = ok(registerPresenceToken(s, TOKEN2, null, T0 + 30))
    s = ok(conf(s, W2, confirm(W2, cg.id, TOKEN, T0 + 40, approval(R2, W2, { issuedAt: T0 })), T0 + 40), 'coguardian_applied')
    expect(s.pending).toEqual([])
    dropped(conf(s, W3, confirm(W3, mv.id, TOKEN2, T0 + 41, approval(R2, W3, { issuedAt: T0 })), T0 + 41), 'not_pending')
    // And R2 can't leave the ward rootless.
    s = ok(inb(s, W2, removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60), 'removal_applied')
    expect(s.roots.map((r) => r.root)).toEqual([R1.pk])
  })
  test('R2 approved as a working key meanwhile: the co-guardian confirm is refused', () => {
    let s = pairedR1W1()
    const cg = coGuardian(R1, R2, { issuedAt: T0 + 10 })
    s = ok(inb(s, W1, cg, T0 + 20))
    s = ok(inb(s, R2, approval(R1, R2, { issuedAt: T0 + 11 }), T0 + 21), 'approval_applied')
    s = ok(registerPresenceToken(s, TOKEN, null, T0 + 30))
    dropped(conf(s, W2, confirm(W2, cg.id, TOKEN, T0 + 40, approval(R2, W2, { issuedAt: T0 })), T0 + 40), 'key_role_conflict')
  })
})

describe('H2: the 72-hour notice runs from the ward receiving the removal', () => {
  test('a backdated removal still waits 72 hours from receipt', () => {
    let s = twoRoots()
    const now = T0 + 10 * DAY
    const rm = removal(R1, R2, { issuedAt: now - H72 - 60 })
    s = ok(inb(s, W1, rm, now), 'removal_pending')
    s = ok(registerPresenceToken(s, TOKEN, dU(R2), now + 1))
    s = ok(conf(s, W1, confirm(W1, rm.id, TOKEN, now + 2), now + 2), 'removal_confirmed')
    expect(projectState(s, now + 2).pending[0]).toMatchObject({ receivedAt: now, takesEffectAt: now + H72 })
    expect(isSigner(tick(s, now + H72 - 1).state, R2.pk)).toBe(true)
  })
})

describe('H3 and P7: objections', () => {
  test('an objection drops a pending removal whatever the two issuedAt', () => {
    let s = twoRoots()
    s = ok(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 300 }), T0 + 100))
    s = ok(inb(s, R2, objection(R2, dU(R2), R1, { issuedAt: T0 + 160 }), T0 + 110), 'objection_applied')
    expect(s.pending).toEqual([])
  })
  test('a removal re-issued after an objection is refused for 7 days', () => {
    let s = twoRoots()
    s = ok(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 100))
    s = ok(inb(s, R2, objection(R2, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 110))
    dropped(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 200 }), T0 + 200), 'objected')
    ok(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 7 * DAY + 110 }), T0 + 7 * DAY + 110), 'removal_pending')
  })
})

describe('H4: the clause floor survives a departure', () => {
  test('a relay cannot restore a superseded clause after the newer signer departs', () => {
    let s = twoRoots()
    const cl = (a: Key, issuedAt: number, now: number) => acceptClause(s, { author: a.pk, sealAuthor: a.pk, subject: 'kid', kind: 'apps', issuedAt }, now)
    s = ok(cl(W1, T0 + 100, T0 + 100))
    s = ok(cl(W2, T0 + 200, T0 + 200))
    s = ok(inb(s, R2, withdrawal(R2, W2, { issuedAt: T0 + 210 }), T0 + 210))
    expect(heldClause(s, 'kid', 'apps')?.floor).toBe(T0 + 200)
    dropped(cl(W1, T0 + 100, T0 + 220), 'not_newer')
    ok(cl(W1, T0 + 201, T0 + 220), 'clause_accepted')
  })
  test('a far-future clause from a departed key is floored at the departure time', () => {
    let s = twoRoots()
    s = ok(acceptClause(s, { author: W2.pk, sealAuthor: W2.pk, subject: 'kid', kind: 'apps', issuedAt: T0 + 500 }, T0 + 200))
    s = ok(inb(s, R2, withdrawal(R2, W2, { issuedAt: T0 + 210 }), T0 + 210))
    expect(heldClause(s, 'kid', 'apps')?.floor).toBe(T0 + 210)
    ok(acceptClause(s, { author: W1.pk, sealAuthor: W1.pk, subject: 'kid', kind: 'apps', issuedAt: T0 + 211 }, T0 + 220))
  })
})

describe('M1: a departed root never returns as a working key', () => {
  test('approval of a removed root is key_departed', () => {
    let s = ok(inb(twoRoots(), W2, removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60))
    s = dropped(inb(s, R2, approval(R1, R2, { issuedAt: T0 + 70 }), T0 + 80), 'key_departed')
    expect(isSigner(s, R2.pk)).toBe(false)
  })
})

describe('M2: strict JSON content', () => {
  const dW = ['d', `kintrinsic/device/${W1.pk}`]
  const pW = ['p', W1.pk]
  const M = ['t', 'charter-device']
  const raw = (c: string) => signRaw(R1, { kind: CHARTER_GUARDIAN_STATEMENT, tags: [dW, pW, M], content: c })
  test.each([
    ['duplicate issuedAt', '{"v":1,"issuedAt":100,"issuedAt":1900000000}'],
    ['v 1.0', '{"v":1.0,"issuedAt":100}'],
    ['issuedAt exponent', '{"v":1,"issuedAt":1e3}'],
    ['issuedAt -0', '{"v":1,"issuedAt":-0}'],
    ['lone surrogate in label', '{"v":1,"issuedAt":100,"label":"\\ud800"}'],
    ['float in an unknown field', '{"v":1,"issuedAt":100,"x":0.5}'],
    ['duplicate in a nested object', '{"v":1,"issuedAt":100,"x":{"a":1,"a":2}}'],
    ['trailing comma', '{"v":1,"issuedAt":100,}'],
  ])('%s is content_not_json', (_n, c) => {
    expect(parseStatement(raw(c))).toEqual({ ok: false, reason: 'content_not_json' })
  })
  test('a surrogate pair and escapes are fine', () => {
    const r = parseStatement(raw('{"v":1,"issuedAt":100,"label":"\\ud83d\\ude00 \\"x\\""}'))
    expect(r.ok && r.statement.label).toBe('😀 "x"')
  })
})

describe('L1: cancels are kept per signer', () => {
  test("another root's held cancel does not shadow the signer's own", () => {
    let s = twoRoots()
    s = ok(inb(s, W2, cancel(R2, dG(R3), { issuedAt: T0 + 400 }), T0 + 150), 'cancel_held')
    s = ok(inb(s, W1, coGuardian(R1, R3, { issuedAt: T0 + 200 }), T0 + 160), 'coguardian_pending')
    s = ok(inb(s, W1, cancel(R1, dG(R3), { issuedAt: T0 + 300 }), T0 + 170), 'cancel_applied')
    expect(s.pending).toEqual([])
  })
})

describe('L3: same-second tie, lowest event id wins', () => {
  test('either arrival order ends with the same label', () => {
    const a = approval(R1, W1b, { issuedAt: T0, label: 'a' })
    const b = approval(R1, W1b, { issuedAt: T0, label: 'b' })
    const [lo, hi] = a.id < b.id ? [a, b] : [b, a]
    const one = ok(inb(ok(inb(pairedR1W1(), W1b, hi, T0 + 10)), W1b, lo, T0 + 11), 'approval_updated')
    const two = inb(ok(inb(pairedR1W1(), W1b, lo, T0 + 10)), W1b, hi, T0 + 11)
    dropped(two, 'tie_not_newer')
    expect(one.keys).toEqual(two.state.keys)
  })
})

describe('L4: binding at confirmation clears other roots’ held withdrawals for W', () => {
  test('held count drops back', () => {
    let s = pairedR1W1()
    s = ok(inb(s, R1, withdrawal(R1, W2, { issuedAt: T0 }), T0 + 5), 'withdrawal_held')
    const cg = coGuardian(R1, R2, { issuedAt: T0 + 5 })
    s = ok(inb(s, W1, cg, T0 + 10))
    s = ok(registerPresenceToken(s, TOKEN, null, T0 + 20))
    s = ok(conf(s, W2, confirm(W2, cg.id, TOKEN, T0 + 30, approval(R2, W2, { issuedAt: T0 })), T0 + 30))
    expect(projectState(s, T0 + 30).held).toBe(0)
    expect(isSigner(s, W2.pk)).toBe(true)
  })
})

describe('L5/L7/M5: RELAYS', () => {
  const rel = (s: GuardianState, a: Key, ts: number, now: number, extra: Partial<Parameters<typeof applyRelays>[1]> = {}) =>
    applyRelays(s, { author: a.pk, sealAuthor: a.pk, channel: 'wrapped', ts, relays: [`wss://r${ts}.example`], ...extra }, now)
  const pin = { v: 1, salt: '00'.repeat(16), hash: 'ab'.repeat(32) }
  test('existing freshness checks: future, >600 s old, before pairing, not newer', () => {
    const s = twoRoots()
    dropped(rel(s, W1, T0 + 101, T0 + 100), 'future')
    dropped(rel(s, W1, T0 + 100, T0 + 701), 'stale')
    dropped(rel(s, W1, T0 - 1, T0 + 10), 'pairing_floor')
    const t = ok(rel(s, W1, T0 + 100, T0 + 100))
    dropped(rel(t, W2, T0 + 100, T0 + 101), 'not_newer')
    dropped(rel(s, W1, T0 + 100, T0 + 100, { sealAuthor: W2.pk }), 'seal_mismatch')
  })
  test('guardianPin only over the wrapped channel', () => {
    const s = twoRoots()
    dropped(rel(s, W1, T0 + 100, T0 + 100, { channel: 'pasted', sealAuthor: null, guardianPin: pin }), 'pin_not_wrapped')
    ok(rel(s, W1, T0 + 100, T0 + 100, { channel: 'pasted', sealAuthor: null }), 'relays_applied')
    dropped(rel(s, W1, T0 + 100, T0 + 100, { guardianPin: { ...pin, v: 2 } }), 'bad_pin_record')
    const t = ok(rel(s, W1, T0 + 100, T0 + 100, { guardianPin: pin }))
    expect(projectState(t, T0 + 100).guardianPin).toEqual(pin)
  })
  test('every list changed within 7 days stays in statusRelays', () => {
    let s = twoRoots()
    s = ok(rel(s, W1, T0 + 100, T0 + 100))
    s = ok(rel(s, W1, T0 + 200, T0 + 200))
    expect(projectState(s, T0 + 200).statusRelays).toEqual([`wss://r${T0 + 200}.example`, `wss://r${T0 + 100}.example`, 'wss://a.example', 'wss://b.example'])
  })
})

describe('timers, tokens and seals', () => {
  test('mutual removals due the same second never leave the ward rootless', () => {
    let s = twoRoots()
    const a = removal(R1, R2, { issuedAt: T0 + 100 })
    const b = removal(R2, R1, { issuedAt: T0 + 100 })
    s = ok(inb(s, W1, a, T0 + 100))
    s = ok(inb(s, W2, b, T0 + 100))
    s = ok(registerPresenceToken(s, TOKEN, null, T0 + 110))
    s = ok(registerPresenceToken(s, TOKEN2, null, T0 + 110))
    s = ok(conf(s, W1, confirm(W1, a.id, TOKEN, T0 + 120), T0 + 120))
    s = ok(conf(s, W2, confirm(W2, b.id, TOKEN2, T0 + 120), T0 + 120))
    const r = tick(s, T0 + 100 + H72)
    expect(r.outcome.ticked).toHaveLength(1)
    expect(r.state.roots).toHaveLength(1)
  })
  test('a token bound to a cancelled d is gone, even if the d is pending again', () => {
    let s = pairedR1W1()
    s = ok(inb(s, W1, coGuardian(R1, R2, { issuedAt: T0 }), T0 + 10))
    s = ok(registerPresenceToken(s, TOKEN, dG(R2), T0 + 20))
    s = ok(inb(s, W1, cancel(R1, dG(R2), { issuedAt: T0 }), T0 + 30))
    const cg = coGuardian(R1, R2, { issuedAt: T0 + 1 })
    s = ok(inb(s, W1, cg, T0 + 40))
    dropped(conf(s, W2, confirm(W2, cg.id, TOKEN, T0 + 50, approval(R2, W2, { issuedAt: T0 })), T0 + 50), 'token_invalid')
  })
  test("a removal signed by R1 sealed by the removed root's W is refused", () => {
    dropped(inb(twoRoots(), W2, removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 110), 'seal_mismatch')
  })
  test('withdrawal of a key that departed with a move, by the re-bind root', () => {
    let s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 }), T0 + 10))
    const mv = move(R1, R3, { issuedAt: T0 + 100 })
    s = ok(inb(s, W1, mv, T0 + 110))
    s = ok(registerPresenceToken(s, TOKEN, null, T0 + 120))
    s = ok(conf(s, W1, confirm(W1, mv.id, TOKEN, T0 + 130, approval(R3, W1, { issuedAt: T0 + 105 })), T0 + 130))
    s = ok(inb(s, W1, withdrawal(R3, W1b, { issuedAt: T0 + 140 }), T0 + 150), 'withdrawal_applied')
    dropped(inb(s, W1b, approval(R3, W1b, { issuedAt: T0 + 160 }), T0 + 170), 'key_departed')
  })
})
