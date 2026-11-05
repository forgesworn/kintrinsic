import { describe, test, expect } from 'vitest'
import {
  applyInbound,
  applyPresenceConfirm,
  registerPresenceToken,
  acceptClause,
  acceptSignedEvent,
  applyRelays,
  markStatusSent,
  tick,
  pairFromOffer,
  pairOfferTemplate,
  signerSet,
  isSigner,
  recipients,
  statusRecipients,
  statusGuardians,
  rootOf,
  projectState,
  heldClause,
  pendingExpiry,
  type GuardianState,
  type SignedEvent,
  type StepResult,
} from '../../src/guardians/index.js'
import {
  R1, R2, R3, R4, R5, W1, W1b, W2, W3, W4, X, T0, DAY, TOKEN, TOKEN2,
  key, sign, approval, withdrawal, coGuardian, removal, move, cancel, objection, confirm,
  pairedR1W1, pairedR1, clauseInput, type Key,
} from './helpers.js'

const H72 = 72 * 3600
const inb = (s: GuardianState, seal: Key, ev: SignedEvent, now = T0 + 10): StepResult =>
  applyInbound(s, { sealAuthor: seal.pk, inner: ev }, now)
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
const dM = (k: Key) => `kintrinsic/root/${k.pk}`

/** R1 (with W1) plus R2 (with W2) pinned through a confirmed co-guardian. */
function twoRoots(): GuardianState {
  let s = pairedR1W1()
  const cg = coGuardian(R1, R2, { issuedAt: T0 + 5 })
  s = ok(inb(s, W1, cg, T0 + 10), 'coguardian_pending')
  s = ok(registerPresenceToken(s, TOKEN2, dG(R2), T0 + 20), 'token_registered')
  const ap = approval(R2, W2, { issuedAt: T0 + 15, label: 'Alex phone' })
  s = ok(applyPresenceConfirm(s, { sealAuthor: W2.pk, inner: confirm(W2, cg.id, TOKEN2, T0 + 30, ap) }, T0 + 30), 'coguardian_applied')
  return s
}

describe('scan-to-pair with a working key', () => {
  const tok = { token: TOKEN, mintedAt: T0 - 10 }
  const offer = (sealer: Key, ap?: SignedEvent, ts = T0) =>
    sign(sealer, pairOfferTemplate({ token: TOKEN, relays: ['wss://a.example'], ts, approval: ap }))

  test('an offer sealed by W with its approval pins R, not W, and binds W', () => {
    const ap = approval(R1, W1, { issuedAt: T0 - 999 })
    const r = pairFromOffer({ sealAuthor: W1.pk, inner: offer(W1, ap) }, tok, T0)
    expect(r.ok).toBe(true)
    if (!r.ok) return
    expect(r.root).toBe(R1.pk)
    expect(r.workingKey).toBe(W1.pk)
    expect(r.state.roots.map((x) => x.root)).toEqual([R1.pk])
    expect(signerSet(r.state)).toEqual([R1.pk, W1.pk].sort())
  })
  test('an offer sealed by a root with no approval pins that root', () => {
    const r = pairFromOffer({ sealAuthor: R1.pk, inner: offer(R1) }, tok, T0)
    expect(r.ok && r.root).toBe(R1.pk)
  })
  test('an approval naming a key other than the sealer is refused', () => {
    const ap = approval(R1, W2, { issuedAt: T0 })
    expect(pairFromOffer({ sealAuthor: W1.pk, inner: offer(W1, ap) }, tok, T0)).toEqual({ ok: false, reason: 'seal_mismatch' })
  })
  test('an invalid approval is refused', () => {
    const ap = { ...approval(R1, W1, { issuedAt: T0 }), sig: '00'.repeat(64) }
    expect(pairFromOffer({ sealAuthor: W1.pk, inner: offer(W1, ap) }, tok, T0)).toEqual({ ok: false, reason: 'bad_approval' })
    const notApproval = withdrawal(R1, W1, { issuedAt: T0 })
    expect(pairFromOffer({ sealAuthor: W1.pk, inner: offer(W1, notApproval) }, tok, T0)).toEqual({ ok: false, reason: 'bad_approval' })
  })
  test('seal author must be the offer author', () => {
    expect(pairFromOffer({ sealAuthor: X.pk, inner: offer(W1) }, tok, T0)).toEqual({ ok: false, reason: 'seal_mismatch' })
  })
  test('token rules: wrong token, expired mint, future ts, stale ts', () => {
    expect(pairFromOffer({ sealAuthor: R1.pk, inner: offer(R1) }, { token: TOKEN2, mintedAt: T0 }, T0).ok).toBe(false)
    expect(pairFromOffer({ sealAuthor: R1.pk, inner: offer(R1) }, { token: TOKEN, mintedAt: T0 - 600 }, T0)).toEqual({ ok: false, reason: 'token_invalid' })
    expect(pairFromOffer({ sealAuthor: R1.pk, inner: offer(R1, undefined, T0 + 1) }, tok, T0)).toEqual({ ok: false, reason: 'token_invalid' })
    expect(pairFromOffer({ sealAuthor: R1.pk, inner: offer(R1, undefined, T0 - 600) }, tok, T0)).toEqual({ ok: false, reason: 'token_invalid' })
  })
  test('an approval with expiresAt is refused', () => {
    const ap = sign(R1, { kind: 31120, created_at: T0, tags: [['d', `kintrinsic/device/${W1.pk}`], ['p', W1.pk], ['t', 'charter-device']], content: JSON.stringify({ v: 1, issuedAt: T0, expiresAt: T0 + 9 }) })
    expect(pairFromOffer({ sealAuthor: W1.pk, inner: offer(W1, ap) }, tok, T0)).toEqual({ ok: false, reason: 'expires_at_not_allowed' })
  })
})

describe('signer set, approvals and the seal exceptions', () => {
  test('approval sealed by the W it names joins W, bound to R', () => {
    const s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 })), 'approval_applied')
    expect(isSigner(s, W1b.pk)).toBe(true)
    expect(s.keys.find((k) => k.key === W1b.pk)?.root).toBe(R1.pk)
  })
  test('exception 1: an approval sealed by anyone else is ignored, even the root', () => {
    dropped(inb(pairedR1W1(), R1, approval(R1, W1b, { issuedAt: T0 })), 'seal_mismatch')
    dropped(inb(pairedR1W1(), W1, approval(R1, W1b, { issuedAt: T0 })), 'seal_mismatch')
  })
  test('an approval for a root that is not pinned is dropped', () => {
    dropped(inb(pairedR1W1(), W2, approval(R2, W2, { issuedAt: T0 })), 'signer_not_pinned')
  })
  test('rule 1: approvals for W from another root are ignored', () => {
    const s = twoRoots()
    dropped(inb(s, W1, approval(R2, W1, { issuedAt: T0 + 50 }), T0 + 60), 'bound_elsewhere')
  })
  test('rule 2: no pairing floor for statements — an approval issued before pairing counts', () => {
    ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 - 30 * DAY })), 'approval_applied')
  })
  test('rule 2: newest issuedAt per d wins; duplicate, stale and same-second tie', () => {
    let s = pairedR1W1()
    const a1 = approval(R1, W1b, { issuedAt: T0, label: 'one' })
    s = ok(inb(s, W1b, a1), 'approval_applied')
    dropped(inb(s, W1b, a1), 'duplicate')
    dropped(inb(s, W1b, approval(R1, W1b, { issuedAt: T0 - 1, label: 'old' })), 'stale')
    dropped(inb(s, W1b, approval(R1, W1b, { issuedAt: T0, label: 'other' })), 'tie_not_newer')
    s = ok(inb(s, W1b, approval(R1, W1b, { issuedAt: T0 + 1, label: 'two' })), 'approval_updated')
    expect(s.keys.find((k) => k.key === W1b.pk)?.label).toBe('two')
  })
  test('a root cannot approve itself or another root as a working key', () => {
    dropped(inb(pairedR1W1(), R1, approval(R1, R1, { issuedAt: T0 })), 'key_role_conflict')
  })
  test('a statement dated ahead of the ward clock is clamped, not refused, and counted', () => {
    const now = T0 + 10
    let s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: now }), now))
    expect(s.clamped).toBe(0)
    s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: now + 301 }), now))
    expect(s.clamped).toBe(1)
    expect(statusGuardians(s).clamped).toBe(1)
    // The signer's issuedAt going backwards is a replay.
    dropped(inb(s, W1b, approval(R1, W1b, { issuedAt: now + 1, label: 'back' }), now + 1), 'replay')
    // Ordering uses the effective time: the signer's next update, a second
    // later by the ward clock, replaces it.
    s = ok(inb(s, W1b, approval(R1, W1b, { issuedAt: now + 302, label: 'two' }), now + 1), 'approval_updated')
    expect(s.clamped).toBe(2)
  })
  test('statements from a non-signer are dropped, not buffered', () => {
    const s = dropped(inb(pairedR1W1(), X, coGuardian(X, R2, { issuedAt: T0 })), 'signer_not_pinned')
    expect(s.pending).toEqual([])
  })
  test('exception 2: a withdrawal signed by R may be sealed by a current W bound to R', () => {
    let s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 })))
    s = ok(inb(s, W1, withdrawal(R1, W1b, { issuedAt: T0 + 1 })), 'withdrawal_applied')
    expect(isSigner(s, W1b.pk)).toBe(false)
  })
  test('exception 2 does not extend to a W bound to another root, or a non-signer', () => {
    const s = twoRoots()
    dropped(inb(s, W2, withdrawal(R1, W1, { issuedAt: T0 + 50 }), T0 + 60), 'seal_mismatch')
    dropped(inb(s, X, withdrawal(R1, W1, { issuedAt: T0 + 50 }), T0 + 60), 'seal_mismatch')
  })
  test('quota: a root has at most 16 active working keys', () => {
    let s = pairedR1W1()
    for (let i = 0; i < 15; i++) {
      const w = key(100 + i)
      s = ok(inb(s, w, approval(R1, w, { issuedAt: T0 })), 'approval_applied')
    }
    const w17 = key(200)
    dropped(inb(s, w17, approval(R1, w17, { issuedAt: T0 })), 'root_key_quota')
    // A withdrawal frees a slot.
    s = ok(inb(s, W1, withdrawal(R1, key(100), { issuedAt: T0 + 1 })))
    ok(inb(s, w17, approval(R1, w17, { issuedAt: T0 })), 'approval_applied')
  })
})

describe('rule 3: withdrawal', () => {
  test('counts only if signed by the binding root', () => {
    const s = twoRoots()
    dropped(inb(s, R2, withdrawal(R2, W1, { issuedAt: T0 + 50 }), T0 + 60), 'not_binding_root')
  })
  test('is permanent: no approval revives W, whatever its issuedAt', () => {
    let s = ok(inb(pairedR1W1(), W1, withdrawal(R1, W1, { issuedAt: T0 })), 'withdrawal_applied')
    dropped(inb(s, W1, approval(R1, W1, { issuedAt: T0 + 300 })), 'key_departed')
    dropped(inb(s, R1, withdrawal(R1, W1, { issuedAt: T0 + 1 })), 'already_withdrawn')
    s = ok(tick(s, T0 + 20))
    expect(s.tombstones).toHaveLength(1)
  })
  test('an early withdrawal from a pinned root is held and pre-empts the approval', () => {
    let s = ok(inb(pairedR1W1(), R1, withdrawal(R1, W1b, { issuedAt: T0 })), 'withdrawal_held')
    expect(projectState(s, T0).held).toBe(1)
    s = ok(inb(s, W1b, approval(R1, W1b, { issuedAt: T0 + 100 })), 'approval_preempted')
    expect(isSigner(s, W1b.pk)).toBe(false)
    expect(s.tombstones).toHaveLength(1)
    expect(projectState(s, T0).held).toBe(0)
  })
  test('same-second tie: the withdrawal wins in either order', () => {
    let s = ok(inb(pairedR1W1(), R1, withdrawal(R1, W1b, { issuedAt: T0 })))
    s = ok(inb(s, W1b, approval(R1, W1b, { issuedAt: T0 })), 'approval_preempted')
    expect(isSigner(s, W1b.pk)).toBe(false)
    let t = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 })))
    t = ok(inb(t, R1, withdrawal(R1, W1b, { issuedAt: T0 })), 'withdrawal_applied')
    expect(isSigner(t, W1b.pk)).toBe(false)
  })
  test('an early withdrawal from an unpinned key is dropped', () => {
    dropped(inb(pairedR1W1(), X, withdrawal(X, W1b, { issuedAt: T0 })), 'signer_not_pinned')
  })
  test('a held withdrawal from another root does not apply once W binds elsewhere', () => {
    let s = twoRoots()
    s = ok(inb(s, R2, withdrawal(R2, W3, { issuedAt: T0 + 40 }), T0 + 50), 'withdrawal_held')
    s = ok(inb(s, W3, approval(R1, W3, { issuedAt: T0 + 41 }), T0 + 50), 'approval_applied')
    expect(isSigner(s, W3.pk)).toBe(true)
    expect(projectState(s, T0 + 50).held).toBe(0)
  })
  test('quota: 64 held early withdrawals per root, and a root at quota cannot approve', () => {
    let s = pairedR1W1()
    for (let i = 0; i < 64; i++) s = ok(inb(s, R1, withdrawal(R1, key(100 + i), { issuedAt: T0 })), 'withdrawal_held')
    dropped(inb(s, R1, withdrawal(R1, key(180), { issuedAt: T0 })), 'held_quota')
    dropped(inb(s, W1b, approval(R1, W1b, { issuedAt: T0 })), 'root_at_quota')
  })
  test('quota: 256 tombstones per root blocks new approvals', () => {
    let s = pairedR1()
    for (let i = 0; i < 256; i++) {
      const w = key(i, 7)
      s = ok(inb(s, w, approval(R1, w, { issuedAt: T0 }), T0 + 10))
      s = ok(inb(s, R1, withdrawal(R1, w, { issuedAt: T0 }), T0 + 10), 'withdrawal_applied')
    }
    expect(s.tombstones).toHaveLength(256)
    const late = key(0, 8)
    dropped(inb(s, late, approval(R1, late, { issuedAt: T0 })), 'root_at_quota')
  }, 60_000) // 512 signed statements: slow on a loaded machine
})

describe('rule 4 and 5: departures fail closed; clauses record their signer', () => {
  const cl = (author: Key, issuedAt: number, seal = author) => clauseInput({ author: author.pk, sealAuthor: seal.pk, subject: 'kid', kind: 'schedule', issuedAt })
  test('a clause from a signer is stored with its signer and root', () => {
    const s = ok(acceptClause(pairedR1W1(), cl(W1, T0 + 1), T0 + 10), 'clause_accepted')
    expect(heldClause(s, 'kid', 'schedule')).toEqual({
      subject: 'kid', kind: 'schedule', signer: W1.pk, root: R1.pk,
      id: cl(W1, T0 + 1).id, issuedAt: T0 + 1, floor: T0 + 1, signerFloors: [{ signer: W1.pk, issuedAt: T0 + 1 }],
    })
  })
  test('a non-signer and a wrong seal are refused; no pairing floor and no future refusal', () => {
    dropped(acceptClause(pairedR1W1(), cl(X, T0 + 1), T0 + 10), 'not_signer')
    dropped(acceptClause(pairedR1W1(), cl(W1, T0 + 1, R1), T0 + 10), 'seal_mismatch')
    ok(acceptClause(pairedR1W1(), cl(W1, T0 - 1), T0 + 10), 'clause_accepted')
    const s = ok(acceptClause(pairedR1W1(), cl(W1, T0 + 311), T0 + 10), 'clause_accepted')
    expect(heldClause(s, 'kid', 'schedule')).toMatchObject({ issuedAt: T0 + 311, floor: T0 + 10, signerFloors: [{ signer: W1.pk, issuedAt: T0 + 311 }] })
    expect(s.clamped).toBe(1)
  })
  test('a clamped clause pins nothing, and its signer\'s floor blocks a replay', () => {
    let s = twoRoots()
    s = ok(acceptClause(s, cl(W1, T0 + DAY), T0 + 100))
    s = ok(acceptClause(s, cl(W2, T0 + 200), T0 + 200))
    const sorted = [{ signer: W1.pk, issuedAt: T0 + DAY }, { signer: W2.pk, issuedAt: T0 + 200 }].sort((a, b) => (a.signer < b.signer ? -1 : 1))
    expect(heldClause(s, 'kid', 'schedule')?.signerFloors).toEqual(sorted)
    dropped(acceptClause(s, cl(W1, T0 + DAY), T0 + 300), 'replay')
    s = ok(acceptClause(s, cl(W2, T0 + DAY), T0 + DAY + 1))
    dropped(acceptClause(s, cl(W1, T0 + DAY), T0 + DAY + 2), 'not_newer')
  })
  test('a clause must be newer than the held clause from a current signer', () => {
    const s = ok(acceptClause(pairedR1W1(), cl(W1, T0 + 5), T0 + 10))
    dropped(acceptClause(s, cl(R1, T0 + 5), T0 + 10), 'not_newer')
    dropped(acceptClause(s, cl(R1, T0 + 4), T0 + 10), 'not_newer')
    ok(acceptClause(s, cl(R1, T0 + 6), T0 + 10))
  })
  test('departed keys sign nothing new; their clauses stay until replaced; their issuedAt is ignored', () => {
    let s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 })))
    s = ok(acceptClause(s, cl(W1b, T0 + 300), T0 + 10)) // far ahead, within skew
    s = ok(inb(s, W1, withdrawal(R1, W1b, { issuedAt: T0 + 20 }), T0 + 20))
    expect(heldClause(s, 'kid', 'schedule')?.signer).toBe(W1b.pk)
    dropped(acceptClause(s, cl(W1b, T0 + 400), T0 + 400), 'not_signer')
    s = ok(acceptClause(s, cl(W1, T0 + 30), T0 + 30), 'clause_accepted')
    expect(heldClause(s, 'kid', 'schedule')?.signer).toBe(W1.pk)
  })
  test('GRANT / USAGE_SYNC: the signer-set gate', () => {
    ok(acceptSignedEvent(pairedR1W1(), { author: W1.pk, sealAuthor: W1.pk, ts: T0 }, T0 + 1), 'signed_accepted')
    dropped(acceptSignedEvent(pairedR1W1(), { author: X.pk, sealAuthor: X.pk, ts: T0 }, T0 + 1), 'not_signer')
    // No time gate here: GRANT keeps its own skew rule; USAGE_SYNC has none.
    ok(acceptSignedEvent(pairedR1W1(), { author: W1.pk, sealAuthor: W1.pk, ts: T0 + DAY }, T0 + 1), 'signed_accepted')
  })
})

describe('rule 6: co-guardian', () => {
  test('pending until presence confirmation by an install bound to R2; then both roots are equal', () => {
    const s = twoRoots()
    expect(s.roots.map((r) => r.root)).toEqual([R1.pk, R2.pk])
    expect(signerSet(s)).toEqual([R1.pk, R2.pk, W1.pk, W2.pk].sort())
    let c = ok(acceptClause(s, clauseInput({ author: W1.pk, sealAuthor: W1.pk, subject: 'k', kind: 'x', issuedAt: T0 + 40 }), T0 + 50))
    c = ok(acceptClause(c, clauseInput({ author: W2.pk, sealAuthor: W2.pk, subject: 'k', kind: 'x', issuedAt: T0 + 41 }), T0 + 50))
    expect(heldClause(c, 'k', 'x')?.root).toBe(R2.pk)
  })
  test('pending is reported in STATUS', () => {
    const cg = coGuardian(R1, R2, { issuedAt: T0 })
    const s = ok(inb(pairedR1W1(), W1, cg))
    expect(statusGuardians(s).pending).toEqual([{ id: cg.id, d: dG(R2), kind: 'coguardian', signer: R1.pk, expiresAt: T0 + 7 * DAY }])
  })
  test('confirmation needs the inline approval, the token, and a W R2 approved', () => {
    const cg = coGuardian(R1, R2, { issuedAt: T0 })
    let s = ok(inb(pairedR1W1(), W1, cg))
    s = ok(registerPresenceToken(s, TOKEN2, dG(R2), T0 + 20))
    const ap = approval(R2, W2, { issuedAt: T0 })
    const now = T0 + 30
    const conf = (w: Key, tok: string, a?: SignedEvent, stmt = cg.id) => applyPresenceConfirm(s, { sealAuthor: w.pk, inner: confirm(w, stmt, tok, now, a) }, now)
    dropped(conf(W2, TOKEN2), 'approval_required')
    dropped(conf(W2, TOKEN, ap), 'token_invalid')
    dropped(conf(W2, TOKEN2, ap, 'ee'.repeat(32)), 'not_pending')
    dropped(conf(W3, TOKEN2, ap), 'bad_approval') // approval names W2, not W3
    dropped(conf(W2, TOKEN2, approval(R3, W2, { issuedAt: T0 })), 'bad_approval') // not R2
    dropped(conf(W1, TOKEN2, approval(R2, W1, { issuedAt: T0 })), 'bound_elsewhere')
    dropped(applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W2, cg.id, TOKEN2, now, ap) }, now), 'seal_mismatch')
    // The token's lifetime is 600 s.
    dropped(applyPresenceConfirm(s, { sealAuthor: W2.pk, inner: confirm(W2, cg.id, TOKEN2, now, ap) }, T0 + 620), 'token_invalid')
    const done = ok(conf(W2, TOKEN2, ap), 'coguardian_applied')
    // Single use.
    expect(done.tokens).toEqual([])
  })
  test('quota: at most 4 roots', () => {
    let s = twoRoots()
    for (const [r, w, n] of [[R3, W3, 3], [R4, W4, 4]] as const) {
      const cg = coGuardian(R1, r, { issuedAt: T0 + 100 + n })
      s = ok(inb(s, W1, cg, T0 + 200))
      s = ok(registerPresenceToken(s, `0${n}`.repeat(16), cg.tags[0]![1]!, T0 + 200))
      s = ok(applyPresenceConfirm(s, { sealAuthor: w.pk, inner: confirm(w, cg.id, `0${n}`.repeat(16), T0 + 200, approval(r, w, { issuedAt: T0 })) }, T0 + 200), 'coguardian_applied')
    }
    expect(s.roots).toHaveLength(4)
    dropped(inb(s, W1, coGuardian(R1, R5, { issuedAt: T0 + 150 }), T0 + 200), 'root_quota')
  })
  test('an already pinned root, or a working key, cannot be a co-guardian', () => {
    dropped(inb(twoRoots(), W1, coGuardian(R1, R2, { issuedAt: T0 + 99 }), T0 + 100), 'already_pinned')
    dropped(inb(pairedR1W1(), R1, coGuardian(R1, W1, { issuedAt: T0 })), 'key_role_conflict')
  })
})

describe('rule 7: root removal', () => {
  test('self-removal takes effect at once', () => {
    const s = ok(inb(twoRoots(), W2, removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60), 'removal_applied')
    expect(s.roots.map((r) => r.root)).toEqual([R1.pk])
    expect(isSigner(s, W2.pk)).toBe(false)
    expect(s.tombstones).toHaveLength(1)
  })
  test('the last root cannot remove itself', () => {
    dropped(inb(pairedR1W1(), W1, removal(R1, R1, { issuedAt: T0 })), 'last_root')
  })
  test('another root: presence by an R1-bound install, then 72 hours from issuedAt', () => {
    let s = twoRoots()
    const rm = removal(R1, R2, { issuedAt: T0 + 100 })
    s = ok(inb(s, W1, rm, T0 + 110), 'removal_pending')
    s = ok(registerPresenceToken(s, TOKEN, dU(R2), T0 + 120))
    dropped(applyPresenceConfirm(s, { sealAuthor: W2.pk, inner: confirm(W2, rm.id, TOKEN, T0 + 130) }, T0 + 130), 'confirmer_not_bound')
    s = ok(applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W1, rm.id, TOKEN, T0 + 130) }, T0 + 130), 'removal_confirmed')
    expect(statusGuardians(s).pending).toEqual([{ id: rm.id, d: dU(R2), kind: 'removal', signer: R1.pk, takesEffectAt: T0 + 110 + H72, expiresAt: T0 + 100 + 7 * DAY }])
    s = tick(s, T0 + 110 + H72 - 1).state
    expect(isSigner(s, R2.pk)).toBe(true)
    const r = tick(s, T0 + 110 + H72)
    expect(r.outcome.ticked).toEqual([{ d: dU(R2), result: 'applied', reason: 'removal_applied' }])
    expect(r.outcome.departed).toEqual([R2.pk, W2.pk].sort())
    expect(isSigner(r.state, W2.pk)).toBe(false)
    expect(statusRecipients(r.state)).toContain(W2.pk) // one last STATUS
    expect(recipients(r.state)).not.toContain(W2.pk)
    expect(markStatusSent(r.state, T0 + 110 + H72).state.farewell).toEqual([])
  })
  test('confirmation after the notice applies at once', () => {
    let s = twoRoots()
    const rm = removal(R1, R2, { issuedAt: T0 + 100 })
    s = ok(inb(s, W1, rm, T0 + 110))
    const late = T0 + 110 + H72 + 5
    s = ok(registerPresenceToken(s, TOKEN, null, late))
    ok(applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W1, rm.id, TOKEN, late) }, late), 'removal_applied')
  })
  test('the removed root can object; that drops the removal', () => {
    let s = twoRoots()
    const rm = removal(R1, R2, { issuedAt: T0 + 100 })
    s = ok(inb(s, W1, rm, T0 + 110))
    const ob = objection(R2, dU(R2), R1, { issuedAt: T0 + 101 })
    dropped(inb(s, W1, ob, T0 + 120), 'seal_mismatch') // W1 is not bound to the objecting root
    dropped(inb(s, R1, objection(R1, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 120), 'objection_mismatch')
    dropped(inb(s, W1, objection(W1, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 120), 'objection_mismatch')
    dropped(inb(s, X, objection(X, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 120), 'signer_not_pinned')
    // An objection naming another remover does not touch this removal; it is held.
    s = ok(inb(s, R2, objection(R2, dU(R2), R3, { issuedAt: T0 + 101 }), T0 + 120), 'objection_held')
    expect(s.pending).toHaveLength(1)
    // Exception 2: a W bound to R2 may seal R2's objection.
    s = ok(inb(s, W2, ob, T0 + 120), 'objection_applied')
    expect(s.pending).toEqual([])
  })
  test("the removed root's working key may sign the objection itself", () => {
    let s = twoRoots()
    s = ok(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 110))
    s = ok(inb(s, W2, objection(W2, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 120), 'objection_applied')
    expect(s.pending).toEqual([])
    // It blocks a re-issued removal for 7 days like the root's own.
    dropped(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 200 }), T0 + 200), 'objected')
  })
  test('a held objection counts against the objecting root, whichever key signed it', () => {
    let s = twoRoots()
    s = ok(inb(s, W2, objection(W2, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 110), 'objection_held')
    expect(projectState(s, T0 + 110).held).toBe(1)
    // A later one in the same slot replaces it, not adds to it.
    s = ok(inb(s, W2, objection(W2, dU(R2), R1, { issuedAt: T0 + 102 }), T0 + 111), 'objection_held')
    expect(projectState(s, T0 + 111).held).toBe(1)
  })
  test('an objection that arrives first drops the removal on arrival', () => {
    let s = twoRoots()
    s = ok(inb(s, R2, objection(R2, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 110), 'objection_held')
    dropped(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 120), 'objected')
    // Within 7 days of the objection, even a newer removal by R1 is refused.
    dropped(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 102 }), T0 + 120), 'objected')
    ok(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 7 * DAY + 200 }), T0 + 7 * DAY + 110), 'removal_pending')
  })
  test('pending statements signed by R2 are dropped when the removal applies', () => {
    let s = twoRoots()
    s = ok(inb(s, W2, coGuardian(R2, R3, { issuedAt: T0 + 90 }), T0 + 100), 'coguardian_pending')
    s = ok(inb(s, W1, removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 110))
    s = ok(registerPresenceToken(s, TOKEN, dU(R2), T0 + 120))
    const rm = s.pending.find((p) => p.d === dU(R2))!
    s = ok(applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W1, rm.id, TOKEN, T0 + 130) }, T0 + 130))
    s = tick(s, T0 + 110 + H72).state
    expect(s.pending).toEqual([])
  })
  test('re-pinning needs a co-guardian newer than the removal; a same-second one loses', () => {
    const s = ok(inb(twoRoots(), W2, removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60))
    dropped(inb(s, W1, coGuardian(R1, R2, { issuedAt: T0 + 50 }), T0 + 70), 'not_newer_than_departure')
    ok(inb(s, W1, coGuardian(R1, R2, { issuedAt: T0 + 51 }), T0 + 70), 'coguardian_pending')
  })
  test('a removal never leaves the ward with no roots', () => {
    // R1 removes R2 (pending, confirmed), then R1 removes itself; the pending removal is refused at its time.
    let s = twoRoots()
    const rm = removal(R1, R2, { issuedAt: T0 + 100 })
    s = ok(inb(s, W1, rm, T0 + 110))
    s = ok(registerPresenceToken(s, TOKEN, dU(R2), T0 + 120))
    s = ok(applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W1, rm.id, TOKEN, T0 + 130) }, T0 + 130))
    // R1 self-removes: R2 remains, and R1's pending removal of R2 is dropped with it.
    s = ok(inb(s, W1, removal(R1, R1, { issuedAt: T0 + 140 }), T0 + 150), 'removal_applied')
    expect(s.roots.map((r) => r.root)).toEqual([R2.pk])
    expect(s.pending).toEqual([])
    dropped(inb(s, W2, removal(R2, R2, { issuedAt: T0 + 160 }), T0 + 170), 'last_root')
  })
})

describe('rule 8: root move', () => {
  function moved() {
    let s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 })))
    const mv = move(R1, R3, { issuedAt: T0 + 100 })
    s = ok(inb(s, W1, mv, T0 + 110), 'move_pending')
    expect(isSigner(s, R1.pk)).toBe(true) // R stays pinned until confirmed
    s = ok(registerPresenceToken(s, TOKEN, dM(R3), T0 + 120))
    // W1 (bound to R1) confirms with R3's approval of itself: it travels with the move.
    const r = applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W1, mv.id, TOKEN, T0 + 130, approval(R3, W1, { issuedAt: T0 + 105 })) }, T0 + 130)
    return { r, s: ok(r, 'move_applied') }
  }
  test('R2 replaces R; the confirming W re-binds; R’s unapproved working keys depart', () => {
    const { r, s } = moved()
    expect(s.roots.map((x) => x.root)).toEqual([R3.pk])
    expect(s.keys.find((k) => k.key === W1.pk)?.root).toBe(R3.pk)
    expect(isSigner(s, W1b.pk)).toBe(false)
    expect(isSigner(s, R1.pk)).toBe(false)
    expect(r.outcome.departed).toEqual([R1.pk, W1b.pk].sort())
  })
  test('the new root may re-bind a key that departed with the move; nobody else may', () => {
    const { s } = moved()
    dropped(inb(s, W1b, approval(R1, W1b, { issuedAt: T0 + 200 }), T0 + 210), 'signer_not_pinned')
    ok(inb(s, W1b, approval(R3, W1b, { issuedAt: T0 + 200 }), T0 + 210), 'approval_rebound')
  })
  test('the old root signs nothing new after the move', () => {
    const { s } = moved()
    dropped(acceptClause(s, clauseInput({ author: R1.pk, sealAuthor: R1.pk, subject: 'k', kind: 'x', issuedAt: T0 + 200 }), T0 + 210), 'not_signer')
  })
  test('a move can be cancelled by R; a co-guardian or move signed by a W is not a root statement', () => {
    let s = ok(inb(pairedR1W1(), W1, move(R1, R3, { issuedAt: T0 + 100 }), T0 + 110))
    dropped(inb(s, W1, move(W1, R3, { issuedAt: T0 + 101 }), T0 + 110), 'signer_not_pinned')
    s = ok(inb(s, W1, cancel(R1, dM(R3), { issuedAt: T0 + 100 }), T0 + 120), 'cancel_applied')
    expect(s.pending).toEqual([])
  })
})

describe('pending statements', () => {
  test('expiresAt is clamped to 7 days; default 7 days', () => {
    expect(pendingExpiry(T0, null)).toBe(T0 + 7 * DAY)
    expect(pendingExpiry(T0, T0 + 30 * DAY)).toBe(T0 + 7 * DAY)
    expect(pendingExpiry(T0, T0 + DAY)).toBe(T0 + DAY)
    const s = ok(inb(pairedR1W1(), W1, coGuardian(R1, R2, { issuedAt: T0, expiresAt: T0 + 30 * DAY })))
    expect(s.pending[0]!.expiresAt).toBe(T0 + 7 * DAY)
  })
  test('expires at its expiresAt on tick', () => {
    const s = ok(inb(pairedR1W1(), W1, coGuardian(R1, R2, { issuedAt: T0, expiresAt: T0 + 100 })))
    expect(tick(s, T0 + 99).state.pending).toHaveLength(1)
    const r = tick(s, T0 + 100)
    expect(r.outcome.ticked).toEqual([{ d: dG(R2), result: 'expired', reason: 'expired' }])
    expect(r.state.pending).toEqual([])
  })
  test('already expired on arrival is dropped', () => {
    dropped(inb(pairedR1W1(), W1, coGuardian(R1, R2, { issuedAt: T0 - 8 * DAY })), 'expired')
  })
  test('a removal whose expiry comes before its notice ends expires instead', () => {
    let s = twoRoots()
    const rm = removal(R1, R2, { issuedAt: T0 + 100, expiresAt: T0 + 100 + DAY })
    s = ok(inb(s, W1, rm, T0 + 110))
    s = ok(registerPresenceToken(s, TOKEN, dU(R2), T0 + 120))
    s = ok(applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W1, rm.id, TOKEN, T0 + 130) }, T0 + 130))
    const r = tick(s, T0 + 100 + H72)
    expect(r.outcome.ticked).toEqual([{ d: dU(R2), result: 'expired', reason: 'expired' }])
    expect(isSigner(r.state, R2.pk)).toBe(true)
  })
  test('cancel drops pending at or below its issuedAt (same-second tie: cancel wins)', () => {
    const s = ok(inb(pairedR1W1(), W1, coGuardian(R1, R2, { issuedAt: T0 })))
    ok(inb(s, W1, cancel(R1, dG(R2), { issuedAt: T0 }), T0 + 20), 'cancel_applied')
  })
  test('a later statement with that d is unaffected', () => {
    let s = ok(inb(pairedR1W1(), W1, coGuardian(R1, R2, { issuedAt: T0 + 5 })))
    s = ok(inb(s, W1, cancel(R1, dG(R2), { issuedAt: T0 + 4 }), T0 + 20), 'cancel_held')
    expect(s.pending).toHaveLength(1)
  })
  test('a cancel that arrives before its statement drops it on arrival', () => {
    const s = ok(inb(pairedR1W1(), R1, cancel(R1, dG(R2), { issuedAt: T0 + 5 })), 'cancel_held')
    dropped(inb(s, W1, coGuardian(R1, R2, { issuedAt: T0 + 5 }), T0 + 20), 'cancelled')
    ok(inb(s, W1, coGuardian(R1, R2, { issuedAt: T0 + 6 }), T0 + 20), 'coguardian_pending')
  })
  test('only the pending statement’s signer can cancel it', () => {
    const s = twoRoots()
    const t = ok(inb(s, W1, coGuardian(R1, R3, { issuedAt: T0 + 50 }), T0 + 60))
    dropped(inb(t, W2, cancel(R2, dG(R3), { issuedAt: T0 + 51 }), T0 + 60), 'cancel_signer_mismatch')
  })
})

describe('rule 9 recipients and STATUS', () => {
  test('a root with no working key receives directly; otherwise its working keys do', () => {
    expect(recipients(pairedR1())).toEqual([R1.pk])
    expect(recipients(pairedR1W1())).toEqual([W1.pk])
    const s = twoRoots()
    expect(recipients(s)).toEqual([W1.pk, W2.pk].sort())
  })
  test('a withdrawn key gets one last STATUS, then nothing', () => {
    let s = ok(inb(pairedR1W1(), W1b, approval(R1, W1b, { issuedAt: T0 })))
    const r = inb(s, W1, withdrawal(R1, W1b, { issuedAt: T0 + 1 }))
    expect(r.outcome.departed).toEqual([W1b.pk])
    s = r.state
    expect(statusRecipients(s)).toContain(W1b.pk)
    s = ok(markStatusSent(s, T0 + 20), 'status_sent')
    expect(statusRecipients(s)).not.toContain(W1b.pk)
  })
  test('STATUS guardians projection', () => {
    const s = twoRoots()
    expect(statusGuardians(s)).toEqual({
      roots: [{ root: R1.pk, keys: [W1.pk] }, { root: R2.pk, keys: [W2.pk] }],
      pending: [],
      tombstones: 0,
      relays: ['wss://a.example', 'wss://b.example'],
      pinSet: false,
      clamped: 0,
    })
  })
  test('rule 11: RELAYS from any signer; a multi-root ward also uses the previous relays for 7 days', () => {
    let s = twoRoots()
    s = ok(applyRelays(s, { author: R2.pk, sealAuthor: R2.pk, channel: 'wrapped', ts: T0 + 100, relays: ['wss://c.example', 'wss://d.example'] }, T0 + 100), 'relays_applied')
    expect(projectState(s, T0 + 100).statusRelays).toEqual(['wss://c.example', 'wss://d.example', 'wss://a.example', 'wss://b.example'])
    expect(projectState(tick(s, T0 + 100 + 7 * DAY).state, T0 + 100 + 7 * DAY).statusRelays).toEqual(['wss://c.example', 'wss://d.example'])
    dropped(applyRelays(s, { author: W1.pk, sealAuthor: W1.pk, ts: T0 + 100, relays: [] }, T0 + 101), 'not_newer')
    dropped(applyRelays(s, { author: X.pk, sealAuthor: X.pk, ts: T0 + 200, relays: [] }, T0 + 201), 'not_signer')
  })
})

describe('state is pure JSON', () => {
  test('round-trips through JSON and the input is never mutated', () => {
    const s = twoRoots()
    const before = JSON.stringify(s)
    inb(s, W1, removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 110)
    expect(JSON.stringify(s)).toBe(before)
    expect(JSON.parse(before)).toEqual(s)
  })
})

describe('rule 11: relay lists, the guardian PIN', () => {
  const pin = { v: 1, salt: '00'.repeat(16), hash: 'ab'.repeat(32) }
  const AB = ['wss://a.example', 'wss://b.example']
  const wrapped = (a: Key, ts: number, extra: { relays?: string[]; guardianPin?: unknown } = {}) =>
    ({ author: a.pk, sealAuthor: a.pk, channel: 'wrapped' as const, ts, ...extra })
  test('only a root may change the relay list; a working key may resend it unchanged', () => {
    const s = twoRoots()
    dropped(applyRelays(s, wrapped(W1, T0 + 100, { relays: ['wss://c.example', 'wss://d.example'] }), T0 + 100), 'relays_root_only')
    dropped(applyRelays(s, { author: W1.pk, sealAuthor: null, channel: 'pasted', ts: T0 + 100, relays: ['wss://c.example', 'wss://d.example'] }, T0 + 100), 'relays_root_only')
    let t = ok(applyRelays(s, wrapped(W1, T0 + 100, { relays: [...AB].reverse(), guardianPin: pin }), T0 + 100), 'relays_applied')
    expect(t.relays).toEqual(AB)
    expect(t.previousRelays).toEqual([])
    expect(statusGuardians(t).pinSet).toBe(true)
    t = ok(applyRelays(t, wrapped(W2, T0 + 101), T0 + 101), 'relays_applied') // battery settings only
    t = ok(applyRelays(t, wrapped(R2, T0 + 102, { relays: ['wss://c.example', 'wss://d.example'] }), T0 + 102), 'relays_applied')
    expect(t.relays).toEqual(['wss://c.example', 'wss://d.example'])
  })
  test('a PIN-only RELAYS is not refused for its age, but keeps the replay floor and the wrap', () => {
    const s = pairedR1W1()
    expect(statusGuardians(s).pinSet).toBe(false)
    const t = ok(applyRelays(s, wrapped(W1, T0 + 100, { guardianPin: pin }), T0 + 100 + DAY), 'relays_applied')
    expect(statusGuardians(t).pinSet).toBe(true)
    expect(t.relaysUpdatedAt).toBe(T0 + 100)
    dropped(applyRelays(t, wrapped(W1, T0 + 100, { guardianPin: pin }), T0 + 100 + DAY), 'not_newer')
    dropped(applyRelays(s, { author: W1.pk, sealAuthor: null, channel: 'pasted', ts: T0 + 100, guardianPin: pin }, T0 + 100), 'pin_not_wrapped')
    dropped(applyRelays(s, wrapped(W1, T0 + 101, { guardianPin: pin }), T0 + 100), 'future')
    // A RELAYS with a list is still held to 600 s.
    dropped(applyRelays(s, wrapped(W1, T0 + 100, { relays: AB, guardianPin: pin }), T0 + 701), 'stale')
  })
})

describe('D5: a phone-held root mints a fresh working key', () => {
  test('R approves a fresh W (W ≠ R, not pinned); W then confirms presence for R\'s own removal of R2', () => {
    // R1 paired directly (the phone holds the root), R2 added with W2.
    let s = pairedR1()
    const cg = coGuardian(R1, R2, { issuedAt: T0 + 5 })
    s = ok(inb(s, R1, cg, T0 + 10), 'coguardian_pending')
    s = ok(registerPresenceToken(s, TOKEN2, dG(R2), T0 + 20))
    s = ok(applyPresenceConfirm(s, { sealAuthor: W2.pk, inner: confirm(W2, cg.id, TOKEN2, T0 + 30, approval(R2, W2, { issuedAt: T0 + 15 })) }, T0 + 30), 'coguardian_applied')
    const rm = removal(R1, R2, { issuedAt: T0 + 40 })
    s = ok(inb(s, R1, rm, T0 + 40), 'removal_pending')
    s = ok(registerPresenceToken(s, TOKEN, dU(R2), T0 + 50))
    // The root itself can't confirm: presence needs a working key bound to it.
    dropped(applyPresenceConfirm(s, { sealAuthor: R1.pk, inner: confirm(R1, rm.id, TOKEN, T0 + 60) }, T0 + 60), 'confirmer_not_bound')
    // The phone mints W1 and R1 approves it: allowed, and W1 binds to R1.
    s = ok(inb(s, W1, approval(R1, W1, { issuedAt: T0 + 61 }), T0 + 61), 'approval_applied')
    expect(rootOf(s, W1.pk)).toBe(R1.pk)
    s = ok(applyPresenceConfirm(s, { sealAuthor: W1.pk, inner: confirm(W1, rm.id, TOKEN, T0 + 62) }, T0 + 62), 'removal_confirmed')
  })
})
