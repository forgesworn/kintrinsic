import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Policy } from "../domain/types";
import { giftToGrant, policyToClauses, timesToGrant } from "./clause";
import { buildTimeExtendGrant } from "./grant";
import { parseRequest } from "./request";
import { parseGrantTimes } from "./times";

// Golden cross-stack vectors: the SAME file is asserted from the Rust consumer
// (core/crates/charter-proto/tests/times_clause_vectors.rs). Here the guardian
// side must agree with the warden about which `times` bodies are valid (a body
// the device drops makes every tagged window LOCK, silently), what a valid one
// reads back as, and how the additive `time` field rides on asks and grants.
// The `composition` section is the warden's own (Free time + a named time -> the
// policy in force); the guardian app has no counterpart to it.
interface Vectors {
  vectors: Array<{
    name: string;
    payload: { v: number; issuedAt: number; times: Array<Record<string, unknown>> };
    expect: { ids?: string[]; runsNothing?: string[]; countsAgainstBudget?: string[] };
  }>;
  invalid: Array<{ name: string; payload: unknown }>;
  params: Array<{
    name: string;
    kind: "gift" | "time.extend.request" | "time.extend.grant";
    payload: Record<string, unknown>;
    expectTime: string | null;
  }>;
}

const VECTORS_PATH = resolve(
  process.cwd(),
  "../../core/crates/charter-testkit/vectors/times/times_clause_vectors.json",
);
const file = JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as Vectors;

const HEX = "ab".repeat(32);

describe("times clause golden vectors (Kintrinsic <-> warden contract)", () => {
  it("the shared vector file is present and non-empty", () => {
    expect(file.vectors.length).toBeGreaterThan(0);
    expect(file.invalid.length).toBeGreaterThan(0);
    expect(file.params.length).toBeGreaterThan(0);
  });

  for (const v of file.vectors) {
    it(`valid: ${v.name} parses, and the ids / runs-nothing / budget verdicts match`, () => {
      const parsed = parseGrantTimes(v.payload);
      expect(parsed).not.toBeNull();
      if (v.expect.ids) {
        expect(parsed!.times.map((t) => t.id)).toEqual(v.expect.ids);
        const runsNothing = parsed!.times
          .filter((t) => t.posture === "onlyThese" && !(t.apps?.length || t.sites?.length))
          .map((t) => t.id);
        expect(runsNothing).toEqual(v.expect.runsNothing ?? []);
        expect(parsed!.times.filter((t) => t.countsAgainstBudget === true).map((t) => t.id)).toEqual(
          v.expect.countsAgainstBudget ?? [],
        );
      }
    });

    it(`valid: ${v.name} survives policyToClauses as a times clause that reads back`, () => {
      const policy: Policy = {
        id: "p",
        scope: { kind: "device" },
        times: v.payload.times as unknown as Policy["times"],
      };
      const clauses = policyToClauses(HEX, policy, v.payload.issuedAt);
      expect(clauses[0].kind).toBe("times");
      // What we emit must itself be a body the device accepts, and the same set.
      expect(parseGrantTimes(clauses[0].body)).toEqual(parseGrantTimes(v.payload));
    });
  }

  for (const v of file.invalid) {
    it(`invalid: ${v.name} fails closed on read-back`, () => {
      expect(parseGrantTimes(v.payload)).toBeNull();
    });

    it(`invalid: ${v.name} is never emitted by the guardian`, () => {
      const p = v.payload as { times?: unknown; issuedAt?: unknown; v?: unknown };
      // A bad envelope (v, issuedAt) is not something timesToGrant is given; the
      // per-time and per-set faults are, and must throw rather than ship.
      if (p.v === 1 && Array.isArray(p.times) && typeof p.issuedAt === "number") {
        expect(() => timesToGrant(p.times as never, p.issuedAt as number)).toThrow();
      }
    });
  }

  for (const v of file.params) {
    it(`params: ${v.name}`, () => {
      if (v.kind === "time.extend.request") {
        const parsed = parseRequest(
          JSON.stringify({
            v: 1,
            op: "time.extend",
            reqId: HEX,
            nonce: HEX,
            subject: HEX,
            machine: HEX,
            ts: 1782734400,
            params: { ...v.payload, minutesRequested: v.payload.minutesRequested },
          }),
        );
        expect(parsed).not.toBeNull();
        expect((parsed!.params as { time?: string }).time ?? null).toBe(v.expectTime);
      } else if (v.kind === "time.extend.grant") {
        const grant = buildTimeExtendGrant({
          reqId: HEX,
          nonce: HEX,
          decision: "allow",
          minutesGranted: v.payload.minutesGranted as number,
          limitHit: v.payload.limitHit as "schedule" | "budget",
          ...(typeof v.payload.time === "string" ? { time: v.payload.time } : {}),
          ts: 1782734400,
        });
        expect(grant.params.time ?? null).toBe(v.expectTime);
        // Absent stays absent: the byte-identical rows have no `time` key at all.
        expect("time" in grant.params).toBe(v.expectTime !== null);
      } else {
        // Assert what THIS side emits for the vector's inputs, not the vector
        // against itself: the gift body the signer sends is `giftToGrant`'s.
        const body = giftToGrant({
          id: "g1",
          issuedAt: 1782734400,
          minutes: (v.payload.minutes as number | undefined) ?? 15,
          expiresAt: 1782777600,
          ...(typeof v.payload.time === "string" ? { time: v.payload.time } : {}),
        });
        expect(body.time ?? null).toBe(v.expectTime);
        expect("time" in body).toBe(v.expectTime !== null);
      }
    });
  }
});
