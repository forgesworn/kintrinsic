import { describe, test, expect } from 'vitest'
import { evaluateSchedule, validateClausePayload, type ChartedClause } from '../../src/schedule/schedule.js'

function baseClause(overrides: Partial<ChartedClause> = {}): ChartedClause {
  return {
    schemaVersion: 1,
    kind: 'schedule',
    mechanism: 'static-data',
    windows: [],
    timezone: 'Europe/London',
    endDate: null,
    revoked: false,
    ...overrides,
  }
}

describe('evaluateSchedule', () => {
  test('revoked clause denies with clause_revoked', () => {
    const result = evaluateSchedule(baseClause({ revoked: true }), new Date())
    expect(result).toEqual({ allow: false, reason: 'clause_revoked' })
  })

  test('expired endDate denies with clause_expired', () => {
    const past = Math.floor(Date.now() / 1000) - 86_400
    const result = evaluateSchedule(baseClause({ endDate: past }), new Date())
    expect(result).toEqual({ allow: false, reason: 'clause_expired' })
  })

  test('future endDate does not trigger expiry', () => {
    const future = Math.floor(Date.now() / 1000) + 86_400
    const result = evaluateSchedule(baseClause({ endDate: future }), new Date())
    expect(result.allow).toBe(true)
  })

  test('empty windows is always_allow', () => {
    const result = evaluateSchedule(baseClause({ windows: [] }), new Date())
    expect(result).toEqual({ allow: true, reason: 'always_allow' })
  })

  test('malformed timezone treated as no_clause (allow)', () => {
    const result = evaluateSchedule(
      baseClause({
        timezone: 'Mars/Olympus_Mons',
        windows: [{ daysOfWeek: [0, 1, 2, 3, 4, 5, 6], startMinute: 0, endMinute: 1439 }],
      }),
      new Date(),
    )
    expect(result).toEqual({ allow: true, reason: 'no_clause' })
  })

  test('window match allows', () => {
    // Tuesday 14:30 UTC
    const now = new Date(Date.UTC(2026, 0, 6, 14, 30))
    const result = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [2], startMinute: 14 * 60, endMinute: 15 * 60 }],
      }),
      now,
    )
    expect(result.allow).toBe(true)
  })

  test('window for wrong day-of-week denies with schedule_locked', () => {
    // Tuesday — windows only allow Mon (1)
    const now = new Date(Date.UTC(2026, 0, 6, 14, 30))
    const result = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [1], startMinute: 0, endMinute: 1439 }],
      }),
      now,
    )
    expect(result).toEqual({ allow: false, reason: 'schedule_locked' })
  })

  test('window matches day but minute outside range denies with schedule_locked', () => {
    // Tuesday 02:00 UTC, window is 14:00-15:00
    const now = new Date(Date.UTC(2026, 0, 6, 2, 0))
    const result = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [2], startMinute: 14 * 60, endMinute: 15 * 60 }],
      }),
      now,
    )
    expect(result).toEqual({ allow: false, reason: 'schedule_locked' })
  })

  test('midnight-crossing window allows on either side of midnight', () => {
    // Friday 23:30 in UTC, window 22:00 (1320) to 06:00 (360) crosses midnight
    const friday2330 = new Date(Date.UTC(2026, 0, 9, 23, 30))
    const r1 = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [5], startMinute: 1320, endMinute: 360 }],
      }),
      friday2330,
    )
    expect(r1.allow).toBe(true)

    // Friday 02:00 same day (00:00–06:00 still in the window from previous Friday's spec)
    const friday0200 = new Date(Date.UTC(2026, 0, 9, 2, 0))
    const r2 = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [5], startMinute: 1320, endMinute: 360 }],
      }),
      friday0200,
    )
    expect(r2.allow).toBe(true)
  })

  test('midnight-crossing window denies in the gap', () => {
    // Friday 12:00 — window is 22:00–06:00, midday is OUT
    const fridayNoon = new Date(Date.UTC(2026, 0, 9, 12, 0))
    const result = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [5], startMinute: 1320, endMinute: 360 }],
      }),
      fridayNoon,
    )
    expect(result).toEqual({ allow: false, reason: 'schedule_locked' })
  })

  test('start-inclusive: minute exactly at startMinute allows', () => {
    const now = new Date(Date.UTC(2026, 0, 6, 14, 0))
    const result = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [2], startMinute: 14 * 60, endMinute: 15 * 60 }],
      }),
      now,
    )
    expect(result.allow).toBe(true)
  })

  test('end-exclusive: minute exactly at endMinute denies', () => {
    const now = new Date(Date.UTC(2026, 0, 6, 15, 0))
    const result = evaluateSchedule(
      baseClause({
        timezone: 'UTC',
        windows: [{ daysOfWeek: [2], startMinute: 14 * 60, endMinute: 15 * 60 }],
      }),
      now,
    )
    expect(result).toEqual({ allow: false, reason: 'schedule_locked' })
  })

  test('revoked beats endDate (revoked first)', () => {
    const past = Math.floor(Date.now() / 1000) - 86_400
    const result = evaluateSchedule(baseClause({ revoked: true, endDate: past }), new Date())
    expect(result.reason).toBe('clause_revoked')
  })

  test('expired beats empty-windows (expired first)', () => {
    const past = Math.floor(Date.now() / 1000) - 86_400
    const result = evaluateSchedule(baseClause({ endDate: past, windows: [] }), new Date())
    expect(result.reason).toBe('clause_expired')
  })
})

describe('validateClausePayload', () => {
  test('accepts a valid baseline clause', () => {
    expect(validateClausePayload(baseClause())).toBe(true)
  })

  test('accepts a clause with one valid window', () => {
    expect(validateClausePayload(baseClause({
      windows: [{ daysOfWeek: [0, 6], startMinute: 0, endMinute: 1439 }],
    }))).toBe(true)
  })

  test('rejects null + non-object', () => {
    expect(validateClausePayload(null)).toBe(false)
    expect(validateClausePayload('schedule')).toBe(false)
    expect(validateClausePayload(42)).toBe(false)
  })

  test('rejects wrong schema version', () => {
    expect(validateClausePayload({ ...baseClause(), schemaVersion: 2 })).toBe(false)
  })

  test('rejects unknown kind', () => {
    expect(validateClausePayload({ ...baseClause(), kind: 'budget' })).toBe(false)
  })

  test('rejects unknown mechanism', () => {
    expect(validateClausePayload({ ...baseClause(), mechanism: 'live-feed' })).toBe(false)
  })

  test('rejects windows as non-array', () => {
    expect(validateClausePayload({ ...baseClause(), windows: 'always' })).toBe(false)
  })

  test('rejects window with out-of-range day', () => {
    expect(validateClausePayload(baseClause({
      windows: [{ daysOfWeek: [7], startMinute: 0, endMinute: 60 }],
    }))).toBe(false)
  })

  test('rejects window with non-integer day', () => {
    expect(validateClausePayload(baseClause({
      windows: [{ daysOfWeek: [1.5], startMinute: 0, endMinute: 60 }],
    }))).toBe(false)
  })

  test('rejects startMinute out of range', () => {
    expect(validateClausePayload(baseClause({
      windows: [{ daysOfWeek: [1], startMinute: -1, endMinute: 60 }],
    }))).toBe(false)
    expect(validateClausePayload(baseClause({
      windows: [{ daysOfWeek: [1], startMinute: 1440, endMinute: 60 }],
    }))).toBe(false)
  })

  test('rejects endMinute out of range', () => {
    expect(validateClausePayload(baseClause({
      windows: [{ daysOfWeek: [1], startMinute: 60, endMinute: 1440 }],
    }))).toBe(false)
  })

  test('rejects empty timezone', () => {
    expect(validateClausePayload({ ...baseClause(), timezone: '' })).toBe(false)
  })

  test('rejects non-string timezone', () => {
    expect(validateClausePayload({ ...baseClause(), timezone: 123 })).toBe(false)
  })

  test('accepts endDate null or unix-number', () => {
    expect(validateClausePayload({ ...baseClause(), endDate: null })).toBe(true)
    expect(validateClausePayload({ ...baseClause(), endDate: 1_900_000_000 })).toBe(true)
  })

  test('rejects endDate as string', () => {
    expect(validateClausePayload({ ...baseClause(), endDate: '1900000000' })).toBe(false)
  })

  test('rejects revoked as non-boolean', () => {
    expect(validateClausePayload({ ...baseClause(), revoked: 'no' })).toBe(false)
  })
})
