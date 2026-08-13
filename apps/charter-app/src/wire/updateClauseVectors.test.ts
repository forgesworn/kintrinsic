import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { updateToGrant } from "./clause";
import type { UpdateManifest } from "./types";

// Golden cross-stack vectors for the `update` clause (#44): the SAME file is
// asserted from the Rust consumer (charter-proto tests/update_clause_vectors.rs
// — UpdateAppBody must parse + validate them). Here we prove the PRODUCER —
// updateToGrant(manifest, url) must emit exactly the pinned wire payload.
interface Vector {
  name: string;
  input: { manifest: UpdateManifest; url: string };
  payload: unknown;
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/update/update_clause_vectors.json",
);
const file = JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as { vectors: Vector[] };

describe("update clause golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(file.vectors.length).toBeGreaterThan(0);
  });

  for (const v of file.vectors) {
    it(`updateToGrant emits the pinned wire payload: ${v.name}`, () => {
      expect(updateToGrant(v.input.manifest, v.input.url)).toEqual(v.payload);
    });
  }
});
