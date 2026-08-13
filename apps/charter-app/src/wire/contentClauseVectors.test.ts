import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { contentToGrant } from "./clause";
import type { WebPolicy } from "../domain/types";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-content/tests/content_clause_vectors.rs). Here we prove
// the PRODUCER — contentToGrant(input) must emit exactly the pinned wire bytes.
interface Vector {
  name: string;
  input: WebPolicy;
  payload: unknown;
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/content/content_clause_vectors.json",
);
const vectors = (
  JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as { vectors: Vector[] }
).vectors;

// contentToGrant stamps issuedAt from its caller; the vectors fix it.
const ISSUED_AT = 1782734400;

describe("content clause golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(vectors.length).toBeGreaterThan(0);
  });

  for (const v of vectors) {
    it(`contentToGrant emits the pinned wire bytes: ${v.name}`, () => {
      expect(contentToGrant(v.input, ISSUED_AT)).toEqual(v.payload);
    });
  }
});
