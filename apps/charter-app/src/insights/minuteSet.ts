// MinuteSet — the 1440-bit day-of-minutes bitmap, byte-identical to the Rust
// side (core/crates/charter-schedule/src/minutes.rs; frozen vectors under
// charter-testkit/vectors/usage_sync/). Bit i = minute i of the local day was
// active; 180 bytes LSB-first within each byte; wire form base64url NO
// padding, exactly 240 chars. This is the union-rule unit: a child's pooled
// screen time is |own ∪ elsewhere| minutes, so simultaneous use across
// devices counts once (spec/contract.md §USAGE_SYNC union-rule extension).

const BYTES = 180;
const B64_CHARS = 240;
export const MINUTES_PER_DAY = 1440;

const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

export function emptyMinutes(): Uint8Array {
  return new Uint8Array(BYTES);
}

/** Mark one minute-of-day (out-of-range is a no-op, mirroring Rust). */
export function setMinute(bits: Uint8Array, minute: number): void {
  if (Number.isInteger(minute) && minute >= 0 && minute < MINUTES_PER_DAY) {
    bits[minute >> 3] |= 1 << (minute & 7);
  }
}

export function hasMinute(bits: Uint8Array, minute: number): boolean {
  return (
    Number.isInteger(minute) &&
    minute >= 0 &&
    minute < MINUTES_PER_DAY &&
    (bits[minute >> 3] & (1 << (minute & 7))) !== 0
  );
}

export function minuteCount(bits: Uint8Array): number {
  let n = 0;
  for (let i = 0; i < BYTES; i++) {
    let b = bits[i];
    while (b) {
      n += b & 1;
      b >>= 1;
    }
  }
  return n;
}

export function isEmptyMinutes(bits: Uint8Array): boolean {
  return bits.every((b) => b === 0);
}

export function unionMinutes(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(BYTES);
  for (let i = 0; i < BYTES; i++) out[i] = a[i] | b[i];
  return out;
}

/** Encode as base64url, no padding: always exactly 240 chars. */
export function encodeMinutes(bits: Uint8Array): string {
  let out = "";
  for (let i = 0; i < BYTES; i += 3) {
    const n = (bits[i] << 16) | (bits[i + 1] << 8) | bits[i + 2];
    out +=
      ALPHABET[(n >> 18) & 63] +
      ALPHABET[(n >> 12) & 63] +
      ALPHABET[(n >> 6) & 63] +
      ALPHABET[n & 63];
  }
  return out;
}

/**
 * Strict decode: exactly 240 base64url chars, standard alphabet only
 * (`-`/`_`, no `+`/`/`, no padding). Anything else is null — malformed wire
 * bitmaps fail closed at the parse boundary.
 */
export function decodeMinutes(s: string): Uint8Array | null {
  if (typeof s !== "string" || s.length !== B64_CHARS) return null;
  const bits = new Uint8Array(BYTES);
  for (let i = 0; i < B64_CHARS; i += 4) {
    const a = ALPHABET.indexOf(s[i]);
    const b = ALPHABET.indexOf(s[i + 1]);
    const c = ALPHABET.indexOf(s[i + 2]);
    const d = ALPHABET.indexOf(s[i + 3]);
    if (a < 0 || b < 0 || c < 0 || d < 0) return null;
    const n = (a << 18) | (b << 12) | (c << 6) | d;
    const o = (i / 4) * 3;
    bits[o] = (n >> 16) & 0xff;
    bits[o + 1] = (n >> 8) & 0xff;
    bits[o + 2] = n & 0xff;
  }
  return bits;
}
