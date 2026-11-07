/**
 * Family roster merge (kind 31119), guardian side only. Wards never see a
 * roster.
 *
 * - Records merge per child: the newest `issuedAt` per `subject` wins.
 * - Each child carries the sender's view of the rules in force, one signed
 *   CLAUSE per `(subject, kind, device)`: family-wide rules in `rules`
 *   (device absent), rules that went to one device in `deviceRules` with that
 *   device's machine. Rules merge on their own, newest `issuedAt` per key (a
 *   tie: the lower event id), whatever the child record did. A ward holds one
 *   clause per kind, so a family-wide rule replaces that kind's older device
 *   rules, and a device rule not newer than the family-wide one is dropped.
 *   A rule is taken only if it verifies and its author is a current signer by
 *   the wards' STATUS (a reported root, or a key listed under one); any other
 *   rule is ignored, not the roster.
 * - A rule the roster doesn't carry is left alone: a roster only adds or
 *   replaces rules, never removes one by leaving it out.
 * - A receiving install never deletes a device it paired itself or has had a
 *   STATUS from.
 * - A device known only from a roster is unconfirmed until a STATUS from it
 *   arrives.
 * - A roster is accepted only from a W approved by a root that the family's
 *   wards report in STATUS, and listed under that root there.
 */
import { hasMarker, isEventShape, parseStatement, verifySignedEvent } from './statements.js'
import { tryParseStrictJson } from './json.js'
import {
  CHARTER_CLAUSE,
  CHARTER_FAMILY_ROSTER,
  FUTURE_SKEW_SECONDS,
  isHex64,
  isUnixSeconds,
  type DeviceRule,
  type Inbound,
  type SignedEvent,
} from './types.js'

export interface LocalRosterDevice {
  machine: string
  platform: string
  name: string
  /** This install paired the device itself. */
  pairedHere: boolean
  /** This install has had a STATUS from the device. */
  statusSeen: boolean
}

export interface LocalRosterChild {
  subject: string
  issuedAt: number
  name: string
  colour: string
  devices: LocalRosterDevice[]
  /**
   * The family-wide rules in force as this install knows them: one signed
   * CLAUSE per clause kind, sorted by kind. Absent in a roster stored before
   * rules travelled; the merge always writes it.
   */
  rules?: SignedEvent[]
  /**
   * The rules that went to one device only: one per `(kind, machine)`, sorted
   * by kind then machine. Absent in a roster stored before device rules
   * travelled; the merge always writes it.
   */
  deviceRules?: DeviceRule[]
}

export interface LocalRoster {
  children: LocalRosterChild[]
}

export type RosterReason =
  | 'bad_event'
  | 'bad_signature'
  | 'wrong_kind'
  | 'seal_mismatch'
  | 'content_not_json'
  | 'bad_version'
  | 'bad_children'
  | 'bad_rules'
  | 'future'
  | 'bad_approval'
  | 'root_not_reported'
  | 'key_not_reported'

export type MergeRosterResult =
  /** `ignoredRules`: rules that did not verify or whose author is not a current signer. */
  | { ok: true; roster: LocalRoster; updated: string[]; ignoredRules: number }
  | { ok: false; reason: RosterReason }

/** A device is confirmed once this install paired it or had a STATUS from it. */
export function isDeviceConfirmed(d: LocalRosterDevice): boolean {
  return d.pairedHere || d.statusSeen
}

function cmp(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0
}

function validChild(x: unknown): x is LocalRosterChild {
  if (typeof x !== 'object' || x === null) return false
  const c = x as Record<string, unknown>
  return (
    isHex64(c.subject) &&
    isUnixSeconds(c.issuedAt) &&
    typeof c.name === 'string' &&
    typeof c.colour === 'string' &&
    Array.isArray(c.devices) &&
    c.devices.every((d) => {
      if (typeof d !== 'object' || d === null) return false
      const o = d as Record<string, unknown>
      return isHex64(o.machine) && typeof o.platform === 'string' && typeof o.name === 'string'
    })
  )
}

/** A CLAUSE as a roster rule: its clause kind and `issuedAt`, or null if it is not one. */
function ruleKey(ev: SignedEvent, subject: string): { kind: string; issuedAt: number } | null {
  if (ev.kind !== CHARTER_CLAUSE || !hasMarker(ev)) return null
  const parsed = tryParseStrictJson(ev.content)
  if (!parsed.ok) return null
  const c = parsed.value
  if (typeof c !== 'object' || c === null || Array.isArray(c)) return null
  const o = c as Record<string, unknown>
  if (typeof o.kind !== 'string' || o.kind === '' || !isUnixSeconds(o.issuedAt)) return null
  // A clause naming another child is not this child's rule; absent is the
  // pairing's sole subject.
  if (o.subject !== undefined && o.subject !== subject) return null
  return { kind: o.kind, issuedAt: o.issuedAt }
}

/** `a` replaces `b`: newer `issuedAt`, or the same and a lower event id. */
function ruleNewer(a: { issuedAt: number; id: string }, b: { issuedAt: number; id: string }): boolean {
  return a.issuedAt > b.issuedAt || (a.issuedAt === b.issuedAt && a.id < b.id)
}

/** A held rule's place in the order `ruleNewer` compares: its `issuedAt` and event id. */
function heldOrder(ev: SignedEvent, subject: string): { issuedAt: number; id: string } | null {
  const k = ruleKey(ev, subject)
  return k ? { issuedAt: k.issuedAt, id: ev.id } : null
}

/** An accepted incoming rule, keyed by `(kind, device)`; `device` absent for a family-wide rule. */
interface AcceptedRule {
  ev: SignedEvent
  kind: string
  issuedAt: number
  device?: string
}

/**
 * Merge accepted rules into one child's held rules, keyed by `(kind, device)`.
 * Returns whether anything changed. A ward holds one clause per kind, so a
 * family-wide rule replaces that kind's device rules it is newer than, and a
 * device rule is taken only if it is newer than that kind's family-wide rule.
 * The result does not depend on the order the rules arrive in.
 */
function mergeRules(holder: LocalRosterChild, accepted: readonly AcceptedRule[]): boolean {
  const subject = holder.subject
  const family = holder.rules ?? []
  let device = holder.deviceRules ?? []
  let changed = false
  const familyOf = (kind: string) => family.findIndex((x) => ruleKey(x, subject)?.kind === kind)
  for (const a of accepted) {
    const mine = { issuedAt: a.issuedAt, id: a.ev.id }
    const fi = familyOf(a.kind)
    const fHeld = fi >= 0 ? heldOrder(family[fi]!, subject) : null
    if (a.device === undefined) {
      if (fHeld && !ruleNewer(mine, fHeld)) continue
      if (fi >= 0) family.splice(fi, 1)
      family.push(a.ev)
      // Device rules of this kind that the family-wide rule is newer than are replaced on every ward.
      device = device.filter((d) => {
        const h = ruleKey(d.rule, subject)?.kind === a.kind ? heldOrder(d.rule, subject) : null
        return !(h && ruleNewer(mine, h))
      })
      changed = true
      continue
    }
    if (fHeld && !ruleNewer(mine, fHeld)) continue
    const di = device.findIndex((d) => d.machine === a.device && ruleKey(d.rule, subject)?.kind === a.kind)
    const dHeld = di >= 0 ? heldOrder(device[di]!.rule, subject) : null
    if (dHeld && !ruleNewer(mine, dHeld)) continue
    if (di >= 0) device.splice(di, 1)
    device.push({ machine: a.device, rule: a.ev })
    changed = true
  }
  const kindOf = (ev: SignedEvent) => ruleKey(ev, subject)?.kind ?? ''
  family.sort((x, y) => cmp(kindOf(x), kindOf(y)))
  device.sort((x, y) => cmp(kindOf(x.rule), kindOf(y.rule)) || cmp(x.machine, y.machine))
  holder.rules = family
  holder.deviceRules = device
  return changed
}

/**
 * Merge an incoming roster into the local one.
 *
 * @param reported the family's wards' STATUS `guardians.roots` (merged
 *   across wards): the sender's approval must be signed by one of these roots
 *   and the sender must be listed under it, so a withdrawn W is refused.
 * @param now the receiving install's clock: a child record more than 300
 *   seconds ahead of it refuses the whole roster.
 */
export function mergeRoster(
  local: LocalRoster,
  inbound: Inbound,
  reported: readonly { root: string; keys: readonly string[] }[],
  now: number,
): MergeRosterResult {
  const inner = inbound.inner
  if (!isEventShape(inner)) return { ok: false, reason: 'bad_event' }
  if (!verifySignedEvent(inner)) return { ok: false, reason: 'bad_signature' }
  if (inner.kind !== CHARTER_FAMILY_ROSTER) return { ok: false, reason: 'wrong_kind' }
  if (inbound.sealAuthor !== inner.pubkey) return { ok: false, reason: 'seal_mismatch' }
  const parsed = tryParseStrictJson(inner.content)
  if (!parsed.ok) return { ok: false, reason: 'content_not_json' }
  const c = parsed.value
  if (typeof c !== 'object' || c === null || Array.isArray(c)) return { ok: false, reason: 'content_not_json' }
  const o = c as Record<string, unknown>
  if (o.v !== 1) return { ok: false, reason: 'bad_version' }
  if (!Array.isArray(o.children) || !o.children.every(validChild)) return { ok: false, reason: 'bad_children' }
  const listOfObjects = (x: unknown) => Array.isArray(x) && x.every((r) => typeof r === 'object' && r !== null && !Array.isArray(r))
  for (const ch of o.children as unknown as Record<string, unknown>[]) {
    if (ch.rules !== undefined && !listOfObjects(ch.rules)) return { ok: false, reason: 'bad_rules' }
    if (ch.deviceRules !== undefined && !listOfObjects(ch.deviceRules)) return { ok: false, reason: 'bad_rules' }
  }
  if ((o.children as LocalRosterChild[]).some((ch) => ch.issuedAt > now + FUTURE_SKEW_SECONDS)) {
    return { ok: false, reason: 'future' }
  }
  const pa = parseStatement(o.approval)
  if (!pa.ok || pa.statement.type !== 'approval' || pa.statement.key !== inner.pubkey || pa.statement.expiresAt !== null) {
    return { ok: false, reason: 'bad_approval' }
  }
  const root = reported.find((r) => r.root === pa.statement.signer)
  if (!root) return { ok: false, reason: 'root_not_reported' }
  if (!root.keys.includes(inner.pubkey)) return { ok: false, reason: 'key_not_reported' }

  // A rule's author must be a current signer by the wards' STATUS.
  const signers = new Set<string>()
  for (const r of reported) {
    signers.add(r.root)
    for (const k of r.keys) signers.add(k)
  }

  const out: LocalRoster = JSON.parse(JSON.stringify(local)) as LocalRoster
  for (const ch of out.children) {
    ch.rules = ch.rules ?? []
    ch.deviceRules = ch.deviceRules ?? []
  }
  const updated = new Set<string>()
  let ignoredRules = 0
  for (const inc of o.children as LocalRosterChild[]) {
    // Rules first: they merge whatever the child record does below.
    const accepted: AcceptedRule[] = []
    const take = (r: unknown, device?: string) => {
      const key = isEventShape(r) && verifySignedEvent(r) && signers.has(r.pubkey) ? ruleKey(r, inc.subject) : null
      if (!key) ignoredRules++
      else accepted.push({ ev: r as SignedEvent, ...key, ...(device !== undefined ? { device } : {}) })
    }
    for (const r of (inc.rules ?? []) as unknown[]) take(r)
    for (const d of (inc.deviceRules ?? []) as unknown as Record<string, unknown>[]) {
      // A device rule must name its machine; one that doesn't is ignored, not the roster.
      if (isHex64(d.machine)) take(d.rule, d.machine)
      else ignoredRules++
    }
    if (accepted.length > 0) {
      let holder = out.children.find((ch) => ch.subject === inc.subject)
      if (!holder) {
        holder = { subject: inc.subject, issuedAt: -1, name: '', colour: '', devices: [], rules: [], deviceRules: [] }
        out.children.push(holder)
      }
      if (mergeRules(holder, accepted)) updated.add(inc.subject)
    }
    const mine = out.children.find((ch) => ch.subject === inc.subject)
    // Newest issuedAt per subject wins; a tie keeps the local record.
    if (mine && mine.issuedAt >= inc.issuedAt) continue
    const devices: LocalRosterDevice[] = inc.devices.map((d) => {
      const known = mine?.devices.find((m) => m.machine === d.machine)
      return {
        machine: d.machine,
        platform: d.platform,
        name: d.name,
        pairedHere: known?.pairedHere ?? false,
        statusSeen: known?.statusSeen ?? false,
      }
    })
    // Never delete a device this install paired or has had a STATUS from.
    for (const m of mine?.devices ?? []) {
      if (isDeviceConfirmed(m) && !devices.some((d) => d.machine === m.machine)) devices.push({ ...m })
    }
    devices.sort((a, b) => cmp(a.machine, b.machine))
    const child: LocalRosterChild = {
      subject: inc.subject,
      issuedAt: inc.issuedAt,
      name: inc.name,
      colour: inc.colour,
      devices,
      rules: mine?.rules ?? [],
      deviceRules: mine?.deviceRules ?? [],
    }
    out.children = out.children.filter((ch) => ch.subject !== inc.subject)
    out.children.push(child)
    updated.add(inc.subject)
  }
  out.children.sort((a, b) => cmp(a.subject, b.subject))
  return { ok: true, roster: out, updated: [...updated].sort(cmp), ignoredRules }
}

/** Record that a STATUS arrived from `machine`: the device is now confirmed. */
export function markDeviceStatusSeen(local: LocalRoster, machine: string): LocalRoster {
  const out: LocalRoster = JSON.parse(JSON.stringify(local)) as LocalRoster
  for (const ch of out.children) for (const d of ch.devices) if (d.machine === machine) d.statusSeen = true
  return out
}
