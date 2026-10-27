import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { debuggingToGrant } from "./clause";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-schedule/tests/debugging_clause_vectors.rs). Here we prove
// the PRODUCER: debuggingToGrant must emit the pinned wire bytes wherever the
// vector is something a guardian can produce. The vectors that are not (a
// future `v`, a stray `until` on a blocked posture, or a malformed body) pin
// what the WARD does with them, so the producer must not emit them at all.
interface Vector {
  name: string;
  payload: { v: number; issuedAt: number; allow: boolean; until?: number };
  expect: { allowedAt: { now: number; allowed: boolean }[] };
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/debugging/debugging_clause_vectors.json",
);
const file = JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as {
  vectors: Vector[];
  invalid: { name: string; payload: unknown }[];
};

describe("debugging clause golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(file.vectors.length).toBeGreaterThan(0);
    expect(file.invalid.length).toBeGreaterThan(0);
  });

  for (const v of file.vectors) {
    const { allow, until, issuedAt } = v.payload;
    if (v.payload.v !== 1) continue; // ward-side only: a producer never emits a future version
    if (!allow && until != null) {
      // The ward ignores a stray `until` on a blocked clause; the producer
      // simply never writes one.
      it(`drops the stray until on a blocked posture: ${v.name}`, () => {
        const g = debuggingToGrant({ allow, until }, issuedAt);
        expect(g).toEqual({ v: 1, issuedAt, allow: false });
        expect(g).not.toHaveProperty("until");
      });
    } else {
      it(`debuggingToGrant emits the pinned wire bytes: ${v.name}`, () => {
        expect(debuggingToGrant({ allow, ...(until != null ? { until } : {}) }, issuedAt)).toEqual(v.payload);
      });
    }
  }
});
