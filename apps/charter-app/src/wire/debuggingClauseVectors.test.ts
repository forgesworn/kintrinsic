import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { debuggingToGrant } from "./clause";
import { debuggingView } from "../domain/debugging";

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

// The guardian's own evaluation. The ward blocks a body whose `v` is not 1, so
// the guardian's reading of a record it could never have sent is blocked too:
// debuggingView takes the record the store keeps, which is only ever written
// from a v1 grant.
describe("debugging clause golden vectors: the guardian's own evaluation (debuggingView)", () => {
  for (const v of file.vectors) {
    const { allow, until, issuedAt } = v.payload;
    for (const { now, allowed } of v.expect.allowedAt) {
      it(`${v.name} at ${now} is ${allowed ? "On" : "Off"}`, () => {
        const record = v.payload.v === 1 ? { allow, ...(until != null ? { until } : {}), issuedAt } : undefined;
        expect(debuggingView(record, now).state === "on").toBe(allowed);
      });
    }
  }
});

// There is no TS parser for a clause body: the guardian only PRODUCES this
// clause (the ward parses it, in Rust, against these same `invalid` payloads).
// So the check available here is that debuggingToGrant can never emit one: for
// every input shape it is handed, the body is a v1 with a boolean `allow`.
describe("debugging clause golden vectors: the producer can never emit an invalid payload", () => {
  const shapes = [
    { allow: false },
    { allow: true },
    { allow: false, until: 5 },
    { allow: true, until: 5 },
    { allow: true, until: 0 },
  ];
  const isInvalid = (g: Record<string, unknown>) =>
    g.v !== 1 || typeof g.allow !== "boolean" || typeof g.issuedAt !== "number";

  it("pins what the invalid payloads are, so a new one is noticed", () => {
    expect(file.invalid.map((i) => i.name)).toEqual(["missing_allow", "allow_not_a_bool", "wrong_version_type"]);
    for (const i of file.invalid) expect(isInvalid(i.payload as Record<string, unknown>), i.name).toBe(true);
  });

  for (const shape of shapes) {
    it(`debuggingToGrant(${JSON.stringify(shape)}) is a well-formed v1 body`, () => {
      const g = debuggingToGrant(shape, 1) as unknown as Record<string, unknown>;
      expect(isInvalid(g)).toBe(false);
      expect(Object.keys(g).sort()).toEqual(shape.allow && shape.until != null ? ["allow", "issuedAt", "until", "v"] : ["allow", "issuedAt", "v"]);
    });
  }
});
