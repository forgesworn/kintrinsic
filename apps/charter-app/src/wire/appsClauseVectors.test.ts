import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { appsToGrant } from "./clause";
import type { AppsPolicy } from "../domain/types";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-proto/tests/apps_clause_vectors.rs). Here we prove the
// PRODUCER — appsToGrant(input) must emit exactly the pinned wire bytes.
interface Vector {
  name: string;
  input: AppsPolicy;
  payload: unknown;
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/apps/apps_clause_vectors.json",
);
const vectors = (
  JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as { vectors: Vector[] }
).vectors;

const ISSUED_AT = 1782734400;

describe("apps clause golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(vectors.length).toBeGreaterThan(0);
  });

  for (const v of vectors) {
    it(`appsToGrant emits the pinned wire bytes: ${v.name}`, () => {
      expect(appsToGrant(v.input, ISSUED_AT)).toEqual(v.payload);
    });
  }
});
