/**
 * The Android guardian PIN: the record a guardian sets inside RELAYS'
 * `guardianPin`, and the wrong-entry backoff.
 *
 * The hash is scrypt (N = 2^15, r = 8, p = 1, 32-byte output) over the PIN's
 * UTF-8 bytes with the record's salt. This SDK carries no scrypt dependency;
 * the golden vectors in `vectors/guardians/pin.json` fix the derivation.
 */

/** scrypt parameters for the guardian PIN hash. */
export const GUARDIAN_PIN_SCRYPT = { N: 32768, r: 8, p: 1, dkLen: 32 } as const

/** Failures allowed before backoff starts. */
export const PIN_FREE_FAILURES = 5
/** First wait, in seconds, after the free failures. */
export const PIN_BACKOFF_BASE_SECONDS = 60
/** Backoff cap, in seconds. */
export const PIN_BACKOFF_CAP_SECONDS = 3600

export interface GuardianPinRecord {
  v: 1
  /** Lowercase hex, 16 to 64 bytes. OQ: Guardian PIN salt and encoding. */
  salt: string
  /** Lowercase hex, 32 bytes. */
  hash: string
}

export type GuardianPinCheck = { ok: true; record: GuardianPinRecord } | { ok: false; reason: 'bad_pin_record' }

/** Validate a `guardianPin` record. */
export function validateGuardianPinRecord(x: unknown): GuardianPinCheck {
  if (typeof x !== 'object' || x === null || Array.isArray(x)) return { ok: false, reason: 'bad_pin_record' }
  const o = x as Record<string, unknown>
  if (o.v !== 1) return { ok: false, reason: 'bad_pin_record' }
  if (typeof o.salt !== 'string' || !/^(?:[0-9a-f]{2}){16,64}$/.test(o.salt)) return { ok: false, reason: 'bad_pin_record' }
  if (typeof o.hash !== 'string' || !/^[0-9a-f]{64}$/.test(o.hash)) return { ok: false, reason: 'bad_pin_record' }
  return { ok: true, record: { v: 1, salt: o.salt, hash: o.hash } }
}

/**
 * Seconds the ward must wait before the next PIN try, given the count of
 * consecutive failures so far: 0 for fewer than 5; after the 5th failure 60 s,
 * doubling with each further failure, capped at 3600 s.
 */
export function pinBackoffSeconds(failures: number): number {
  if (!Number.isSafeInteger(failures) || failures < PIN_FREE_FAILURES) return 0
  const exp = failures - PIN_FREE_FAILURES
  if (exp >= 6) return PIN_BACKOFF_CAP_SECONDS
  return Math.min(PIN_BACKOFF_BASE_SECONDS * 2 ** exp, PIN_BACKOFF_CAP_SECONDS)
}

/** The earliest Unix second the next try is allowed, from the last failure's time. */
export function pinNextTryAt(failures: number, lastFailureAt: number): number {
  return lastFailureAt + pinBackoffSeconds(failures)
}
