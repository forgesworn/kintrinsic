/**
 * One time-window in a Charter schedule clause.
 *
 * `endMinute <= startMinute` means the window crosses midnight (e.g.
 * `{startMinute: 1320, endMinute: 360, daysOfWeek: [5]}` = Friday 22:00
 * through Saturday 06:00).
 */
export interface ScheduleWindow {
  /** 0 = Sunday, 6 = Saturday. */
  daysOfWeek: number[]
  /** Minute-of-day in the clause's timezone (0..1439). */
  startMinute: number
  /** Minute-of-day in the clause's timezone (0..1439). */
  endMinute: number
}

/**
 * A Charter clause payload (decrypted, parsed). Currently `schedule` is the
 * only `kind`; future clauses may add `budget`, `spend`, `content`, `comms`.
 */
export interface ChartedClause {
  schemaVersion: 1
  kind: 'schedule'
  mechanism: 'static-data'
  windows: ScheduleWindow[]
  /** IANA timezone identifier, e.g. `Europe/London`. */
  timezone: string
  /** Optional hard-stop in unix seconds. */
  endDate: number | null
  /** Soft tombstone — parent explicitly revoked the clause. */
  revoked: boolean
}

export interface EvaluateScheduleResult {
  allow: boolean
  reason?:
    | 'clause_revoked'
    | 'clause_expired'
    | 'always_allow'
    | 'no_clause'
    | 'schedule_locked'
}

const DAY_INDEX: Record<string, number> = {
  Sun: 0,
  Mon: 1,
  Tue: 2,
  Wed: 3,
  Thu: 4,
  Fri: 5,
  Sat: 6,
}

function isValidTimezone(tz: string): boolean {
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: tz }).format(new Date())
    return true
  } catch {
    return false
  }
}

function dayMinuteInTz(now: Date, tz: string): { day: number; minute: number } {
  const fmt = new Intl.DateTimeFormat('en-US', {
    timeZone: tz,
    weekday: 'short',
    hour: 'numeric',
    minute: 'numeric',
    hour12: false,
  })
  let day = -1
  let hour = 0
  let minute = 0
  for (const p of fmt.formatToParts(now)) {
    if (p.type === 'weekday') day = DAY_INDEX[p.value] ?? -1
    else if (p.type === 'hour') hour = parseInt(p.value, 10) % 24
    else if (p.type === 'minute') minute = parseInt(p.value, 10)
  }
  return { day, minute: hour * 60 + minute }
}

/**
 * Evaluate a Charter schedule clause against the current moment.
 *
 * Pure: no IO, no globals. The clause's `timezone` determines what
 * "now" means for window matching.
 *
 * Returns `{allow: true}` when the clause is either always-allow (empty
 * windows), or `now` falls inside at least one window for `now`'s day-of-week.
 * Returns `{allow: false, reason}` for revoked, expired, or
 * schedule_locked outcomes.
 *
 * Malformed timezones return `{allow: true, reason: 'no_clause'}` per
 * Charter rev. 7: a broken clause is treated as the absence of a clause
 * (fail open at the lib level — consumers can layer stricter policy).
 */
export function evaluateSchedule(
  payload: ChartedClause,
  now: Date,
): EvaluateScheduleResult {
  if (payload.revoked) return { allow: false, reason: 'clause_revoked' }

  if (payload.endDate !== null && payload.endDate !== undefined) {
    if (Math.floor(now.getTime() / 1000) > payload.endDate) {
      return { allow: false, reason: 'clause_expired' }
    }
  }

  if (payload.windows.length === 0) {
    return { allow: true, reason: 'always_allow' }
  }

  if (!isValidTimezone(payload.timezone)) {
    return { allow: true, reason: 'no_clause' }
  }

  const { day, minute } = dayMinuteInTz(now, payload.timezone)
  for (const w of payload.windows) {
    if (!w.daysOfWeek.includes(day)) continue
    if (w.endMinute > w.startMinute) {
      if (minute >= w.startMinute && minute < w.endMinute) return { allow: true }
    } else {
      // Window crosses midnight (endMinute <= startMinute).
      if (minute >= w.startMinute || minute < w.endMinute) return { allow: true }
    }
  }

  return { allow: false, reason: 'schedule_locked' }
}

/**
 * Structural type-guard for `ChartedClause`. Use to reject malformed
 * inbound payloads before passing to {@link evaluateSchedule}.
 *
 * Strict: rejects unknown schema versions, wrong-typed fields, out-of-range
 * minutes, and non-integer day-of-week values.
 */
export function validateClausePayload(payload: unknown): payload is ChartedClause {
  if (!payload || typeof payload !== 'object') return false
  const p = payload as Record<string, unknown>
  if (p.schemaVersion !== 1) return false
  if (p.kind !== 'schedule') return false
  if (p.mechanism !== 'static-data') return false
  if (!Array.isArray(p.windows)) return false

  for (const w of p.windows as unknown[]) {
    if (!w || typeof w !== 'object') return false
    const win = w as Record<string, unknown>
    if (!Array.isArray(win.daysOfWeek)) return false
    for (const d of win.daysOfWeek) {
      if (typeof d !== 'number' || !Number.isInteger(d) || d < 0 || d > 6) return false
    }
    if (typeof win.startMinute !== 'number' || win.startMinute < 0 || win.startMinute > 1439) return false
    if (typeof win.endMinute !== 'number' || win.endMinute < 0 || win.endMinute > 1439) return false
  }

  if (typeof p.timezone !== 'string' || p.timezone.length === 0) return false
  if (p.endDate !== null && p.endDate !== undefined && typeof p.endDate !== 'number') return false
  if (typeof p.revoked !== 'boolean') return false

  return true
}
