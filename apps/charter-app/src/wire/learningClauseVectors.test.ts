import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { learningToGrant } from "./clause";
import type { LearningPolicy } from "../domain/types";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-proto/tests/learning_clause_vectors.rs). Here we prove
// the PRODUCER — learningToGrant(input) must emit exactly the pinned wire bytes.
interface Vector {
  name: string;
  input: LearningPolicy;
  payload: unknown;
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/learning/learning_clause_vectors.json",
);
const vectors = (
  JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as { vectors: Vector[] }
).vectors;

const ISSUED_AT = 1782734400;

describe("learning clause golden vectors (Kintrinsic ↔ warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(vectors.length).toBeGreaterThan(0);
  });

  for (const v of vectors) {
    it(`learningToGrant emits the pinned wire bytes: ${v.name}`, () => {
      expect(learningToGrant(v.input, ISSUED_AT)).toEqual(v.payload);
    });
  }
});

// A costing site lives in the `learning` clause ONLY because that clause is
// where a pinned, resolver-locked window is defined — its time never came from
// there. So pausing "free times", which pauses the free GRANT, must leave it
// alone. Dropping it would silently delete its launcher AND make its window
// unsanctioned, so the site-app lockdown would terminate it mid-use: a
// guardian pausing one thing, closing a completely different one, with no
// message anywhere. Mirrors `GrantLearning::defined_apps` on the device.
describe("pausing free times does not un-define a costing site's window", () => {
  const khan = {
    id: "khan-academy",
    label: "Khan Academy",
    kind: "site" as const,
    url: "https://www.khanacademy.org/",
    domains: ["khanacademy.org"],
  };
  const youtube = {
    id: "youtube",
    label: "YouTube",
    kind: "site" as const,
    url: "https://www.youtube.com/",
    domains: ["youtube.com"],
    free: false,
  };

  it("keeps the costing entry and drops the free one when paused", () => {
    const g = learningToGrant({ enabled: false, apps: [khan, youtube] }, ISSUED_AT);
    expect(g.paused).toBe(true);
    expect(g.apps.map((a) => a.id)).toEqual(["youtube"]);
  });

  it("a family with no costing sites still sends an empty paused clause", () => {
    const g = learningToGrant({ enabled: false, apps: [khan] }, ISSUED_AT);
    expect(g).toEqual({ v: 1, apps: [], paused: true, issuedAt: ISSUED_AT });
  });

  it("carries everything when not paused", () => {
    const g = learningToGrant({ enabled: true, apps: [khan, youtube] }, ISSUED_AT);
    expect(g.apps.map((a) => a.id)).toEqual(["khan-academy", "youtube"]);
  });
});
