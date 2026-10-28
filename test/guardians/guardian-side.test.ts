import { describe, test, expect } from 'vitest'
import {
  approvalTemplate,
  buildApproval,
  cancelTemplate,
  coGuardianTemplate,
  familyRosterTemplate,
  mergeRoster,
  markDeviceStatusSeen,
  isDeviceConfirmed,
  objectionTemplate,
  pairOfferContent,
  parseStatement,
  pinBackoffSeconds,
  pinNextTryAt,
  secretKeySigner,
  signTemplate,
  validateGuardianPinRecord,
  withdrawalTemplate,
  type LocalRoster,
  type EventTemplate,
} from '../../src/guardians/index.js'
import { R1, R2, W1, W2, X, T0, sign, approval } from './helpers.js'

describe('builders', () => {
  test('templates are deterministic and created_at defaults to issuedAt', () => {
    const t = approvalTemplate(W1.pk, { issuedAt: T0, label: 'Sam phone' })
    expect(t).toEqual({
      kind: 31120,
      created_at: T0,
      tags: [['d', `kintrinsic/device/${W1.pk}`], ['p', W1.pk], ['t', 'charter-device']],
      content: JSON.stringify({ v: 1, issuedAt: T0, label: 'Sam phone' }),
    })
    expect(approvalTemplate(W1.pk, { issuedAt: T0, label: 'Sam phone' })).toEqual(t)
  })
  test('builders refuse bad input', () => {
    expect(() => approvalTemplate(W1.pk.toUpperCase(), { issuedAt: T0 })).toThrow()
    expect(() => approvalTemplate(W1.pk, { issuedAt: T0, label: 'x'.repeat(65) })).toThrow()
    expect(() => approvalTemplate(W1.pk, { issuedAt: T0, expiresAt: T0 + 1 })).toThrow()
    expect(() => withdrawalTemplate(W1.pk, { issuedAt: T0, expiresAt: T0 + 1 })).toThrow()
    expect(() => cancelTemplate(`kintrinsic/device/${W1.pk}`, { issuedAt: T0 })).toThrow()
    expect(() => objectionTemplate(`kintrinsic/guardian/${R2.pk}`, R1.pk, { issuedAt: T0 })).toThrow()
  })
  test('signing is pluggable: a key and an async remote signer give valid statements', async () => {
    const viaKey = await buildApproval(R1.sk, W1.pk, { issuedAt: T0 })
    expect(parseStatement(viaKey).ok).toBe(true)
    const remote = async (t: EventTemplate) => secretKeySigner(R1.sk)(t)
    const viaRemote = await signTemplate(coGuardianTemplate(R2.pk, { issuedAt: T0 }), remote)
    const r = parseStatement(viaRemote)
    expect(r.ok && r.statement.signer).toBe(R1.pk)
  })
  test('PAIR_OFFER content carries the approval when sealed by a working key', () => {
    const ap = approval(R1, W1, { issuedAt: T0 })
    const c = JSON.parse(pairOfferContent({ token: 'ab'.repeat(16), relays: ['wss://a.example'], ts: T0, approval: ap }))
    expect(c.approval).toEqual(JSON.parse(JSON.stringify(ap)))
    expect(JSON.parse(pairOfferContent({ token: 'ab'.repeat(16), relays: [], ts: T0 })).approval).toBeUndefined()
  })
})

describe('family roster merge', () => {
  const REP = [{ root: R2.pk, keys: [W2.pk] }]
  const kid = 'aa'.repeat(32)
  const dev = (m: string) => ({ machine: m.repeat(32), platform: 'android', name: `dev ${m}` })
  const roster = (issuedAt: number, devices: ReturnType<typeof dev>[], from = W2, root = R2) =>
    sign(from, familyRosterTemplate({ children: [{ subject: kid, issuedAt, name: 'Robin', colour: '#123456', devices }], approval: approval(root, from, { issuedAt: T0 }), createdAt: T0 }))
  const local: LocalRoster = {
    children: [{
      subject: kid, issuedAt: T0, name: 'Rob', colour: '#000000',
      devices: [
        { machine: '11'.repeat(32), platform: 'linux', name: 'laptop', pairedHere: true, statusSeen: false },
        { machine: '22'.repeat(32), platform: 'android', name: 'tablet', pairedHere: false, statusSeen: true },
        { machine: '33'.repeat(32), platform: 'android', name: 'old', pairedHere: false, statusSeen: false },
      ],
    }],
  }
  test('newest issuedAt per subject wins; self-paired and STATUS-seen devices are never deleted', () => {
    const r = mergeRoster(local, { sealAuthor: W2.pk, inner: roster(T0 + 1, [dev('44')]) }, [{ root: R1.pk, keys: [W1.pk] }, { root: R2.pk, keys: [W2.pk] }], T0)
    expect(r.ok).toBe(true)
    if (!r.ok) return
    const ch = r.roster.children[0]!
    expect(ch.name).toBe('Robin')
    expect(ch.devices.map((d) => d.machine)).toEqual(['11'.repeat(32), '22'.repeat(32), '44'.repeat(32)])
    const added = ch.devices.find((d) => d.machine === '44'.repeat(32))!
    expect(isDeviceConfirmed(added)).toBe(false)
    expect(isDeviceConfirmed(markDeviceStatusSeen(r.roster, added.machine).children[0]!.devices.find((d) => d.machine === added.machine)!)).toBe(true)
  })
  test('an older or same-second record does not replace the local one', () => {
    const r = mergeRoster(local, { sealAuthor: W2.pk, inner: roster(T0, [dev('44')]) }, REP, T0)
    expect(r.ok && r.roster).toEqual(local)
  })
  test('only from a W approved by a root the wards report, sealed by that W', () => {
    expect(mergeRoster(local, { sealAuthor: W2.pk, inner: roster(T0 + 1, []) }, [{ root: R1.pk, keys: [W2.pk] }], T0)).toEqual({ ok: false, reason: 'root_not_reported' })
    // A withdrawn W is no longer listed under its root.
    expect(mergeRoster(local, { sealAuthor: W2.pk, inner: roster(T0 + 1, []) }, [{ root: R2.pk, keys: [] }], T0)).toEqual({ ok: false, reason: 'key_not_reported' })
    expect(mergeRoster(local, { sealAuthor: W2.pk, inner: roster(T0 + 301, []) }, REP, T0)).toEqual({ ok: false, reason: 'future' })
    expect(mergeRoster(local, { sealAuthor: W2.pk, inner: roster(T0 + 300, []) }, REP, T0).ok).toBe(true)
    expect(mergeRoster(local, { sealAuthor: X.pk, inner: roster(T0 + 1, []) }, REP, T0)).toEqual({ ok: false, reason: 'seal_mismatch' })
    const wrongApproval = sign(W2, familyRosterTemplate({ children: [], approval: approval(R2, W1, { issuedAt: T0 }), createdAt: T0 }))
    expect(mergeRoster(local, { sealAuthor: W2.pk, inner: wrongApproval }, REP, T0)).toEqual({ ok: false, reason: 'bad_approval' })
  })
})

describe('guardian PIN', () => {
  test('record validation', () => {
    expect(validateGuardianPinRecord({ v: 1, salt: '00'.repeat(16), hash: 'ab'.repeat(32) }).ok).toBe(true)
    expect(validateGuardianPinRecord({ v: 2, salt: '00'.repeat(16), hash: 'ab'.repeat(32) }).ok).toBe(false)
    expect(validateGuardianPinRecord({ v: 1, salt: '00'.repeat(8), hash: 'ab'.repeat(32) }).ok).toBe(false)
    expect(validateGuardianPinRecord({ v: 1, salt: '00'.repeat(16), hash: 'AB'.repeat(32) }).ok).toBe(false)
    expect(validateGuardianPinRecord(null).ok).toBe(false)
  })
  test('backoff: free for 4, then 1 minute after the 5th, doubling to a 1-hour cap', () => {
    expect([0, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 100].map(pinBackoffSeconds)).toEqual([0, 0, 0, 60, 120, 240, 480, 960, 1920, 3600, 3600, 3600])
    expect(pinNextTryAt(5, T0)).toBe(T0 + 60)
  })
})
