import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { tetheringToGrant } from "./clause";
import type { Tethering } from "../domain/types";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-schedule/tests/tethering_clause_vectors.rs). Here we prove
// the PRODUCER — tetheringToGrant(input) must emit exactly the pinned wire bytes.
interface Vector {
  name: string;
  input: Tethering;
  payload: unknown;
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/tethering/tethering_clause_vectors.json",
);
const vectors = (
  JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as { vectors: Vector[] }
).vectors;

const ISSUED_AT = 1782734400;

describe("tethering clause golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(vectors.length).toBeGreaterThan(0);
  });

  for (const v of vectors) {
    it(`tetheringToGrant emits the pinned wire bytes: ${v.name}`, () => {
      expect(tetheringToGrant(v.input, ISSUED_AT)).toEqual(v.payload);
    });
  }
});
