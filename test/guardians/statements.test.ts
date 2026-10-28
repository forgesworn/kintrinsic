import { describe, test, expect } from 'vitest'
import { parseStatement, CHARTER_GUARDIAN_STATEMENT } from '../../src/guardians/index.js'
import { R1, R2, W1, T0, approval, withdrawal, coGuardian, removal, move, cancel, objection, signRaw } from './helpers.js'

const M = ['t', 'charter-device']
const content = (o: Record<string, unknown>) => JSON.stringify(o)
const raw = (tags: string[][], c: string, kind = CHARTER_GUARDIAN_STATEMENT) => signRaw(R1, { kind, tags, content: c })
const good = { v: 1, issuedAt: T0 }

describe('parseStatement: the seven types', () => {
  const o = { issuedAt: T0 }
  test.each([
    ['approval', approval(R1, W1, { ...o, label: 'Sam phone' }), W1.pk],
    ['withdrawal', withdrawal(R1, W1, o), W1.pk],
    ['coguardian', coGuardian(R1, R2, o), R2.pk],
    ['removal', removal(R1, R2, o), R2.pk],
    ['move', move(R1, R2, o), R2.pk],
    ['cancel', cancel(R1, `kintrinsic/guardian/${R2.pk}`, o), R2.pk],
    ['objection', objection(R2, `kintrinsic/unguardian/${R2.pk}`, R1, o), R2.pk],
  ])('%s parses', (type, ev, k) => {
    const r = parseStatement(ev)
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.statement.type).toBe(type)
    expect(r.statement.key).toBe(k)
    expect(r.statement.issuedAt).toBe(T0)
  })

  test('cancel and objection carry their target', () => {
    const c = parseStatement(cancel(R1, `kintrinsic/root/${R2.pk}`, { issuedAt: T0 }))
    expect(c.ok && c.statement.target).toBe(`kintrinsic/root/${R2.pk}`)
    expect(c.ok && c.statement.targetType).toBe('move')
    const ob = parseStatement(objection(R2, `kintrinsic/unguardian/${R2.pk}`, R1, { issuedAt: T0 }))
    expect(ob.ok && ob.statement.p).toBe(R1.pk)
  })

  test('unknown content fields are ignored and created_at plays no part', () => {
    const ev = raw([['d', `kintrinsic/device/${W1.pk}`], ['p', W1.pk], M], content({ ...good, extra: { x: 1 } }))
    const r = parseStatement({ ...ev })
    expect(r.ok).toBe(true)
    const ev2 = signRaw(R1, { kind: CHARTER_GUARDIAN_STATEMENT, tags: [['d', `kintrinsic/device/${W1.pk}`], ['p', W1.pk], M], content: content(good), created_at: 5 })
    const r2 = parseStatement(ev2)
    expect(r2.ok && r2.statement.issuedAt).toBe(T0)
  })

  test('a 64-byte label is accepted, multi-byte included', () => {
    expect(parseStatement(approval(R1, W1, { issuedAt: T0, label: 'a'.repeat(64) })).ok).toBe(true)
    const ev = raw([['d', `kintrinsic/device/${W1.pk}`], ['p', W1.pk], M], content({ ...good, label: 'é'.repeat(32) }))
    expect(parseStatement(ev).ok).toBe(true)
  })
})

describe('parseStatement: malformed statements are ignored', () => {
  const dW = ['d', `kintrinsic/device/${W1.pk}`]
  const pW = ['p', W1.pk]
  const cases: [string, unknown, string][] = [
    ['not an object', 'nope', 'bad_event'],
    ['tampered content', { ...approval(R1, W1, { issuedAt: T0 }), content: content({ v: 1, issuedAt: T0 + 1 }) }, 'bad_signature'],
    ['tampered sig', { ...approval(R1, W1, { issuedAt: T0 }), sig: '00'.repeat(64) }, 'bad_signature'],
    ['wrong kind', raw([dW, pW, M], content(good), 30078), 'wrong_kind'],
    ['missing marker', raw([dW, pW], content(good)), 'missing_marker'],
    ['two d tags', raw([dW, dW, pW, M], content(good)), 'd_tag_count'],
    ['no d tag', raw([pW, M], content(good)), 'd_tag_count'],
    ['two p tags', raw([dW, pW, pW, M], content(good)), 'p_tag_count'],
    ['no p tag', raw([dW, M], content(good)), 'p_tag_count'],
    ['unknown prefix', raw([['d', `kindependence/device/${W1.pk}`], pW, M], content(good)), 'unknown_d_prefix'],
    ['uppercase key', raw([['d', `kintrinsic/device/${W1.pk.toUpperCase()}`], pW, M], content(good)), 'bad_key'],
    ['short p', raw([dW, ['p', 'abcd'], M], content(good)), 'bad_key'],
    ['d/p mismatch', raw([dW, ['p', R2.pk], M], content(good)), 'd_p_mismatch'],
    ['cancel of an approval', raw([['d', `kintrinsic/cancel/kintrinsic/device/${W1.pk}`], pW, M], content(good)), 'bad_cancel_target'],
    ['cancel d/p mismatch', raw([['d', `kintrinsic/cancel/kintrinsic/guardian/${R2.pk}`], pW, M], content(good)), 'd_p_mismatch'],
    ['objection of a co-guardian', raw([['d', `kintrinsic/object/kintrinsic/guardian/${R2.pk}`], ['p', R1.pk], M], content(good)), 'bad_objection_target'],
    ['content not JSON', raw([dW, pW, M], 'not json'), 'content_not_json'],
    ['content an array', raw([dW, pW, M], '[1]'), 'content_not_json'],
    ['v 2', raw([dW, pW, M], content({ v: 2, issuedAt: T0 })), 'bad_version'],
    ['v missing', raw([dW, pW, M], content({ issuedAt: T0 })), 'bad_version'],
    ['v "1"', raw([dW, pW, M], content({ v: '1', issuedAt: T0 })), 'bad_version'],
    ['issuedAt missing', raw([dW, pW, M], content({ v: 1 })), 'bad_issued_at'],
    ['issuedAt fractional', raw([dW, pW, M], content({ v: 1, issuedAt: T0 + 0.5 })), 'bad_issued_at'],
    ['issuedAt negative', raw([dW, pW, M], content({ v: 1, issuedAt: -1 })), 'bad_issued_at'],
    ['label 65 bytes', raw([dW, pW, M], content({ ...good, label: 'a'.repeat(65) })), 'bad_label'],
    ['label 66 bytes multi-byte', raw([dW, pW, M], content({ ...good, label: 'é'.repeat(33) })), 'bad_label'],
    ['label not a string', raw([dW, pW, M], content({ ...good, label: 5 })), 'bad_label'],
    ['expiresAt not an integer', raw([dW, pW, M], content({ ...good, expiresAt: 'soon' })), 'bad_expires_at'],
  ]
  test.each(cases)('%s', (_name, ev, reason) => {
    expect(parseStatement(ev)).toEqual({ ok: false, reason })
  })
})
