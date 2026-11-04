import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { connectionsToGrant, policyToClauses } from "./clause";
import type { Connections, Policy } from "../domain/types";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-schedule/tests/connections_clause_vectors.rs). Here we
// prove the PRODUCER: connectionsToGrant(tiers) emits exactly the pinned bytes
// for every v1 vector.
interface Vector {
  name: string;
  payload: { v: number; issuedAt: number; wifi: string; bluetooth: string };
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/connections/connections_clause_vectors.json",
);
const vectors = (
  JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as { vectors: Vector[] }
).vectors.filter((v) => v.payload.v === 1);

describe("connections clause golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(vectors.length).toBeGreaterThan(0);
  });

  for (const v of vectors) {
    it(`connectionsToGrant emits the pinned wire bytes: ${v.name}`, () => {
      const input = { wifi: v.payload.wifi, bluetooth: v.payload.bluetooth } as Connections;
      expect(connectionsToGrant(input, v.payload.issuedAt)).toEqual(v.payload);
    });
  }

  it("a policy without connections signs no connections clause", () => {
    const policy: Policy = { id: "p", scope: { kind: "device" } };
    expect(policyToClauses(null, policy, 1).some((c) => c.kind === "connections")).toBe(false);
    const withIt: Policy = { ...policy, connections: { wifi: "secured", bluetooth: "any" } };
    const c = policyToClauses(null, withIt, 1).find((x) => x.kind === "connections");
    expect(c?.body).toEqual({ v: 1, issuedAt: 1, wifi: "secured", bluetooth: "any" });
  });
});
