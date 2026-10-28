/**
 * Strict JSON for guardian-key content (statements, presence confirmations,
 * pair offers, rosters).
 *
 * Every ward must read one signed event the same way, whatever its JSON
 * library. So on top of RFC 8259 this parser refuses:
 * - a duplicate member name in any object (libraries differ on which copy
 *   wins, or throw);
 * - any number token that is not an integer: no fraction, no exponent, no
 *   `-0` (libraries differ on `1.0` and `1e3` for integer fields);
 * - a lone UTF-16 surrogate in any string, escaped or raw (not valid Unicode,
 *   and refused by strict decoders).
 * - nesting deeper than MAX_JSON_DEPTH (16; objects and arrays combined, the
 *   top-level value being depth 1): decoders differ in their recursion limits,
 *   so the contract fixes one.
 * Integers beyond 2^53 − 1 parse (they may sit in ignored fields) but lose
 * precision; every field the contract reads is range-checked separately.
 */

export class StrictJsonError extends Error {}

/** Deepest nesting accepted: objects and arrays combined, the top-level value is depth 1. */
export const MAX_JSON_DEPTH = 16

const WS = new Set([0x20, 0x09, 0x0a, 0x0d])

export function parseStrictJson(text: string): unknown {
  let i = 0
  let depth = 0
  const enter = () => {
    if (++depth > MAX_JSON_DEPTH) fail('nesting too deep')
  }
  const fail = (msg: string): never => {
    throw new StrictJsonError(`${msg} at ${i}`)
  }
  const ws = () => {
    while (i < text.length && WS.has(text.charCodeAt(i))) i++
  }
  const value = (): unknown => {
    ws()
    const c = text[i]
    if (c === '{') return object()
    if (c === '[') return array()
    if (c === '"') return string()
    if (c === 't') return literal('true', true)
    if (c === 'f') return literal('false', false)
    if (c === 'n') return literal('null', null)
    if (c === '-' || (c !== undefined && c >= '0' && c <= '9')) return number()
    return fail('unexpected character')
  }
  const literal = (word: string, v: unknown) => {
    if (text.startsWith(word, i)) {
      i += word.length
      return v
    }
    return fail('bad literal')
  }
  const number = () => {
    const m = /^-?(?:0|[1-9][0-9]*)/.exec(text.slice(i))
    if (!m) return fail('bad number')
    const tok = m[0]
    const next = text[i + tok.length]
    if (next === '.' || next === 'e' || next === 'E') return fail('non-integer number')
    if (tok === '-0') return fail('negative zero')
    i += tok.length
    return Number(tok)
  }
  const string = () => {
    i++ // opening quote
    let out = ''
    for (;;) {
      if (i >= text.length) return fail('unterminated string')
      const code = text.charCodeAt(i)
      if (code === 0x22) {
        i++
        break
      }
      if (code < 0x20) return fail('control character in string')
      if (code === 0x5c) {
        const e = text[i + 1]
        i += 2
        if (e === '"') out += '"'
        else if (e === '\\') out += '\\'
        else if (e === '/') out += '/'
        else if (e === 'b') out += '\b'
        else if (e === 'f') out += '\f'
        else if (e === 'n') out += '\n'
        else if (e === 'r') out += '\r'
        else if (e === 't') out += '\t'
        else if (e === 'u') {
          const hex = text.slice(i, i + 4)
          if (!/^[0-9a-fA-F]{4}$/.test(hex)) return fail('bad unicode escape')
          out += String.fromCharCode(parseInt(hex, 16))
          i += 4
        } else return fail('bad escape')
        continue
      }
      out += text[i]
      i++
    }
    for (let j = 0; j < out.length; j++) {
      const u = out.charCodeAt(j)
      if (u >= 0xd800 && u <= 0xdbff) {
        const lo = out.charCodeAt(j + 1)
        if (!(lo >= 0xdc00 && lo <= 0xdfff)) return fail('lone surrogate')
        j++
      } else if (u >= 0xdc00 && u <= 0xdfff) {
        return fail('lone surrogate')
      }
    }
    return out
  }
  const array = () => {
    enter()
    i++
    const out: unknown[] = []
    ws()
    if (text[i] === ']') {
      i++
      depth--
      return out
    }
    for (;;) {
      out.push(value())
      ws()
      if (text[i] === ',') {
        i++
        continue
      }
      if (text[i] === ']') {
        i++
        depth--
        return out
      }
      return fail('bad array')
    }
  }
  const object = () => {
    enter()
    i++
    const out: Record<string, unknown> = Object.create(null) as Record<string, unknown>
    const seen = new Set<string>()
    ws()
    if (text[i] === '}') {
      i++
      depth--
      return { ...out }
    }
    for (;;) {
      ws()
      if (text[i] !== '"') return fail('expected member name')
      const k = string()
      if (seen.has(k)) return fail('duplicate member name')
      seen.add(k)
      ws()
      if (text[i] !== ':') return fail('expected colon')
      i++
      Object.defineProperty(out, k, { value: value(), enumerable: true, writable: true, configurable: true })
      ws()
      if (text[i] === ',') {
        i++
        continue
      }
      if (text[i] === '}') {
        i++
        depth--
        return { ...out }
      }
      return fail('bad object')
    }
  }
  const v = value()
  ws()
  if (i !== text.length) fail('trailing characters')
  return v
}

/** Parse strictly without throwing. */
export function tryParseStrictJson(text: string): { ok: true; value: unknown } | { ok: false } {
  try {
    return { ok: true, value: parseStrictJson(text) }
  } catch {
    return { ok: false }
  }
}
