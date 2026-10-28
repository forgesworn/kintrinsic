/**
 * Family roster merge (kind 31119), guardian side only. Wards never see a
 * roster.
 *
 * - Records merge per child: the newest `issuedAt` per `subject` wins.
 * - A receiving install never deletes a device it paired itself or has had a
 *   STATUS from.
 * - A device known only from a roster is unconfirmed until a STATUS from it
 *   arrives.
 * - A roster is accepted only from a W approved by a root that the family's
 *   wards report in STATUS.
 */
import { isEventShape, parseStatement, verifySignedEvent } from './statements.js'
import { CHARTER_FAMILY_ROSTER, isHex64, isUnixSeconds, type Inbound } from './types.js'

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
  | 'bad_approval'
  | 'root_not_reported'

export type MergeRosterResult =
  | { ok: true; roster: LocalRoster; updated: string[] }
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

/**
 * Merge an incoming roster into the local one.
 *
 * @param reportedRoots the roots the family's wards report in STATUS
 *   `guardians.roots`; the sender's approval must be signed by one of them.
 */
export function mergeRoster(local: LocalRoster, inbound: Inbound, reportedRoots: readonly string[]): MergeRosterResult {
  const inner = inbound.inner
  if (!isEventShape(inner)) return { ok: false, reason: 'bad_event' }
  if (!verifySignedEvent(inner)) return { ok: false, reason: 'bad_signature' }
  if (inner.kind !== CHARTER_FAMILY_ROSTER) return { ok: false, reason: 'wrong_kind' }
  if (inbound.sealAuthor !== inner.pubkey) return { ok: false, reason: 'seal_mismatch' }
  let c: unknown
  try {
    c = JSON.parse(inner.content)
  } catch {
    return { ok: false, reason: 'content_not_json' }
  }
  if (typeof c !== 'object' || c === null || Array.isArray(c)) return { ok: false, reason: 'content_not_json' }
  const o = c as Record<string, unknown>
  if (o.v !== 1) return { ok: false, reason: 'bad_version' }
  if (!Array.isArray(o.children) || !o.children.every(validChild)) return { ok: false, reason: 'bad_children' }
  const pa = parseStatement(o.approval)
  if (!pa.ok || pa.statement.type !== 'approval' || pa.statement.key !== inner.pubkey || pa.statement.expiresAt !== null) {
    return { ok: false, reason: 'bad_approval' }
  }
  if (!reportedRoots.includes(pa.statement.signer)) return { ok: false, reason: 'root_not_reported' }

  const out: LocalRoster = JSON.parse(JSON.stringify(local)) as LocalRoster
  const updated: string[] = []
  for (const inc of o.children as LocalRosterChild[]) {
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
    const child: LocalRosterChild = { subject: inc.subject, issuedAt: inc.issuedAt, name: inc.name, colour: inc.colour, devices }
    out.children = out.children.filter((ch) => ch.subject !== inc.subject)
    out.children.push(child)
    updated.push(inc.subject)
  }
  out.children.sort((a, b) => cmp(a.subject, b.subject))
  return { ok: true, roster: out, updated: updated.sort(cmp) }
}

/** Record that a STATUS arrived from `machine`: the device is now confirmed. */
export function markDeviceStatusSeen(local: LocalRoster, machine: string): LocalRoster {
  const out: LocalRoster = JSON.parse(JSON.stringify(local)) as LocalRoster
  for (const ch of out.children) for (const d of ch.devices) if (d.machine === machine) d.statusSeen = true
  return out
}
