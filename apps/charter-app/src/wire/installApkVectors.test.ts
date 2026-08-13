import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { buildInstallApkGrant, type InstallApkDecision } from "./grant";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-proto/tests/install_apk_vectors.rs). Here we prove the
// PRODUCER — buildInstallApkGrant(input) must emit exactly the pinned bytes, so
// a serialization drift in Kintrinsic fails against the shared contract.
interface Vector {
  name: string;
  input: InstallApkDecision;
  payload: unknown;
}

// Vitest runs with cwd at this package root (apps/charter-app); the shared
// vector file lives two levels up under core/.
const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/grant/install_apk_vectors.json",
);
const vectors = (
  JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as { vectors: Vector[] }
).vectors;

describe("install.apk grant golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(vectors.length).toBeGreaterThan(0);
  });

  for (const v of vectors) {
    it(`buildInstallApkGrant emits the pinned wire bytes: ${v.name}`, () => {
      expect(buildInstallApkGrant(v.input)).toEqual(v.payload);
    });
  }
});
