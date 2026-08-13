// Frozen cross-stack MinuteSet vectors — the SAME file the Rust side asserts
// (charter-schedule/tests/minute_set_vectors.rs), so codec drift breaks
// loudly on both stacks.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  decodeMinutes,
  emptyMinutes,
  encodeMinutes,
  minuteCount,
  setMinute,
  unionMinutes,
} from "./minuteSet";

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/usage_sync/minute_set_vectors.json",
);

type MinutesSpec = number[] | string; // [1,2,3] or "range:a-b" (inclusive)
interface Doc {
  vectors: { name: string; minutes: MinutesSpec; b64url: string }[];
  unions: {
    name: string;
    a: MinutesSpec;
    b: MinutesSpec;
    aB64url?: string;
    bB64url?: string;
    unionCount: number;
  }[];
  invalid: string[];
}

const doc = JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as Doc;

function minutesOf(spec: MinutesSpec): number[] {
  if (Array.isArray(spec)) return spec;
  const [a, b] = spec.replace("range:", "").split("-").map(Number);
  return Array.from({ length: b - a + 1 }, (_, i) => a + i);
}

function bitsOf(spec: MinutesSpec): Uint8Array {
  const bits = emptyMinutes();
  for (const m of minutesOf(spec)) setMinute(bits, m);
  return bits;
}

describe("minuteSet frozen vectors", () => {
  it("has vectors", () => {
    expect(doc.vectors.length).toBeGreaterThan(0);
    expect(doc.invalid.length).toBeGreaterThan(0);
  });

  for (const v of doc.vectors) {
    it(`encodes + decodes pinned bytes: ${v.name}`, () => {
      const bits = bitsOf(v.minutes);
      expect(encodeMinutes(bits)).toBe(v.b64url);
      expect(decodeMinutes(v.b64url)).toEqual(bits);
    });
  }

  for (const u of doc.unions) {
    it(`union counts overlap once: ${u.name}`, () => {
      const a = bitsOf(u.a);
      const b = bitsOf(u.b);
      expect(minuteCount(unionMinutes(a, b))).toBe(u.unionCount);
      if (u.aB64url) expect(encodeMinutes(a)).toBe(u.aB64url);
      if (u.bB64url) expect(encodeMinutes(b)).toBe(u.bB64url);
    });
  }

  for (const s of doc.invalid) {
    it(`rejects invalid: ${JSON.stringify(s).slice(0, 24)}…`, () => {
      expect(decodeMinutes(s)).toBeNull();
    });
  }
});
