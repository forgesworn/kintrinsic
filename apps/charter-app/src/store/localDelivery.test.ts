import { describe, expect, it } from "vitest";
import {
  MAX_DELIVERY_AGE_MS,
  MAX_DELIVERY_RECORDS,
  addRecord,
  answerDeliveryLine,
  answeredLines,
  appliedSuffix,
  applyStates,
  idsToPoll,
  isFinalState,
  linkRecords,
  pruneRecords,
  recordsOf,
  ruleAppliedMachines,
  type LocalWrapRecord,
} from "./localDelivery";

const M1 = "a".repeat(64);
const M2 = "b".repeat(64);
const REQ = "r".repeat(64);
const NOW = 1_800_000_000_000;

function rec(over: Partial<LocalWrapRecord>): LocalWrapRecord {
  return { wrap: "w", machine: M1, kind: "clause", at: NOW, state: "sent", ...over };
}
const nameOf = (r: LocalWrapRecord) => (r.machine === M1 ? "Robin's laptop" : "Mia's phone");

describe("the delivery record", () => {
  it("is bounded to 200 entries and 7 days, keeping the newest", () => {
    let list: LocalWrapRecord[] = [];
    for (let i = 0; i < MAX_DELIVERY_RECORDS + 25; i++) list = addRecord(list, rec({ wrap: `w${i}`, at: NOW + i }), NOW + i);
    expect(list).toHaveLength(MAX_DELIVERY_RECORDS);
    expect(list[0].wrap).toBe("w25");
    expect(list[list.length - 1].wrap).toBe(`w${MAX_DELIVERY_RECORDS + 24}`);

    const old = [rec({ wrap: "old", at: NOW - MAX_DELIVERY_AGE_MS - 1 }), rec({ wrap: "new" })];
    expect(pruneRecords(old, NOW).map((r) => r.wrap)).toEqual(["new"]);
  });

  it("re-recording a wrap replaces it rather than duplicating", () => {
    const list = addRecord([rec({ wrap: "w", state: "queued" })], rec({ wrap: "w", state: "sent" }), NOW);
    expect(list).toHaveLength(1);
    expect(list[0].state).toBe("sent");
  });

  it("reads a damaged persisted slice defensively", () => {
    expect(recordsOf(undefined)).toEqual([]);
    expect(recordsOf("nope")).toEqual([]);
    expect(recordsOf([rec({}), { wrap: 1 }, null, rec({ state: "bogus" as never }), rec({ kind: "x" as never })])).toHaveLength(1);
  });

  it("folds carrier states in, returns the same array when nothing changed, and never undoes a final state", () => {
    const list = [rec({ wrap: "a", state: "sent" }), rec({ wrap: "b", state: "applied" })];
    expect(applyStates(list, {})).toBe(list);
    expect(applyStates(list, { a: { state: "sent" } })).toBe(list);
    const next = applyStates(list, { a: { state: "received" }, b: { state: "sent" } });
    expect(next.map((r) => r.state)).toEqual(["received", "applied"]);
    expect(applyStates(next, { a: { state: "applied" } })[0].state).toBe("applied");
  });

  it("polls only non-final, recent entries", () => {
    const list = [
      rec({ wrap: "queued", state: "queued" }),
      rec({ wrap: "sent", state: "sent" }),
      rec({ wrap: "received", state: "received" }),
      rec({ wrap: "done", state: "applied" }),
      rec({ wrap: "rejected", state: "rejected" }),
      rec({ wrap: "stale", state: "queued", at: NOW - 49 * 3600 * 1000 }),
    ];
    expect(idsToPoll(list, NOW)).toEqual(["queued", "sent", "received"]);
    expect(["queued", "sent", "received"].some((s) => isFinalState(s as never))).toBe(false);
  });

  it("links wraps to an ask once, sharing the array when nothing changes", () => {
    const list = [rec({ wrap: "c1" }), rec({ wrap: "c2" })];
    const linked = linkRecords(list, ["c1"], REQ);
    expect(linked[0].reqId).toBe(REQ);
    expect(linked[1].reqId).toBeUndefined();
    expect(linkRecords(linked, ["c1"], REQ)).toBe(linked);
  });
});

describe("answer lines", () => {
  const grant = (state: LocalWrapRecord["state"], over: Partial<LocalWrapRecord> = {}) =>
    rec({ wrap: "g", kind: "grant", reqId: REQ, state, ...over });
  const clause = (state: LocalWrapRecord["state"], over: Partial<LocalWrapRecord> = {}) =>
    rec({ wrap: "c", kind: "clause", reqId: REQ, state, ...over });
  const ask = (kind: string, status = "approved") => ({ reqId: REQ, kind, status });

  it("time.extend: 'Applied on {device}' once the grant is applied or unchanged", () => {
    expect(answerDeliveryLine(ask("time.extend"), [grant("applied")], nameOf)).toEqual({
      tone: "ok",
      text: "Applied on Robin's laptop",
    });
    expect(answerDeliveryLine(ask("time.extend"), [grant("unchanged")], nameOf)?.text).toBe("Applied on Robin's laptop");
  });

  it("says nothing it cannot back: not applied until the ward says so", () => {
    for (const s of ["queued", "sent", "received"] as const) {
      const line = answerDeliveryLine(ask("time.extend"), [grant(s)], nameOf)!;
      expect(line.tone).toBe("info");
      expect(line.text).not.toMatch(/Applied/);
    }
    expect(answerDeliveryLine(ask("time.extend"), [grant("queued")], nameOf)!.text).toBe(
      "Waiting for Robin's laptop to join the home network.",
    );
    expect(answerDeliveryLine(ask("time.extend"), [], nameOf)).toBeNull();
    expect(answerDeliveryLine({ kind: "time.extend", status: "approved" }, [grant("applied")], nameOf)).toBeNull();
  });

  it("app.open is applied only when the grant AND the same device's apps clause both are", () => {
    const open = ask("app.open");
    expect(answerDeliveryLine(open, [grant("applied"), clause("sent")], nameOf)).toMatchObject({ tone: "info" });
    expect(answerDeliveryLine(open, [grant("sent"), clause("applied")], nameOf)).toMatchObject({ tone: "info" });
    expect(answerDeliveryLine(open, [grant("applied"), clause("applied")], nameOf)).toEqual({
      tone: "ok",
      text: "Applied on Robin's laptop",
    });
    expect(answerDeliveryLine(open, [grant("unchanged"), clause("unchanged")], nameOf)?.text).toBe("Applied on Robin's laptop");
  });

  it("app.open ignores another device's clause, and says nothing when no clause was tracked", () => {
    const open = ask("app.open");
    const other = clause("applied", { wrap: "c-other", machine: M2 });
    expect(answerDeliveryLine(open, [grant("applied"), other], nameOf)).toBeNull();
    expect(answerDeliveryLine(open, [grant("applied")], nameOf)).toBeNull();
    // …and the other device's pending clause does not hold this one back.
    expect(
      answerDeliveryLine(open, [grant("applied"), clause("applied"), clause("queued", { wrap: "c2", machine: M2 })], nameOf)?.text,
    ).toBe("Applied on Robin's laptop");
  });

  it("an app.open denial is just the grant", () => {
    expect(answerDeliveryLine(ask("app.open", "denied"), [grant("applied")], nameOf)).toEqual({
      tone: "ok",
      text: "Robin's laptop has your answer.",
    });
  });

  it("install approvals say 'Approved on', never 'installed'", () => {
    const line = answerDeliveryLine(ask("install.app"), [grant("applied")], nameOf)!;
    expect(line.text).toBe("Approved on Robin's laptop");
    expect(line.text).not.toMatch(/install/i);
  });

  it("shows expired, refused and failed as a plain warning line", () => {
    expect(answerDeliveryLine(ask("time.extend"), [grant("expired")], nameOf)).toEqual({
      tone: "warn",
      text: "Robin's laptop didn't act on this: the ask had already expired there.",
    });
    expect(answerDeliveryLine(ask("time.extend"), [grant("refused")], nameOf)).toEqual({
      tone: "warn",
      text: "Robin's laptop didn't accept this answer.",
    });
    expect(answerDeliveryLine(ask("time.extend"), [grant("failed")], nameOf)?.tone).toBe("warn");
    // A clause failure on app.open is a warning even though the grant landed.
    expect(answerDeliveryLine(ask("app.open"), [grant("applied"), clause("failed")], nameOf)?.tone).toBe("warn");
  });

  it("lists recent answered asks newest first, only when they travelled the home network", () => {
    const reqs = [
      { id: "r1", reqId: "q1", kind: "time.extend", status: "approved" },
      { id: "r2", reqId: "q2", kind: "time.extend", status: "approved" },
      { id: "r3", reqId: "q3", kind: "time.extend", status: "pending" },
      { id: "r4", reqId: "q4", kind: "time.extend", status: "approved" },
    ];
    const recs = [
      rec({ wrap: "g1", kind: "grant", reqId: "q1", state: "applied", at: NOW - 5000 }),
      rec({ wrap: "g2", kind: "grant", reqId: "q2", state: "applied", at: NOW - 1000 }),
      rec({ wrap: "g3", kind: "grant", reqId: "q3", state: "applied", at: NOW }),
    ];
    expect(answeredLines(reqs, recs, nameOf, NOW).map((x) => x.req.id)).toEqual(["r2", "r1"]);
    expect(answeredLines(reqs, recs, nameOf, NOW + 25 * 3600 * 1000)).toEqual([]);
  });
});

describe("rule changes", () => {
  it("names the devices where every tracked clause wrap of the change was applied", () => {
    const records = [
      rec({ wrap: "c1", machine: M1, state: "applied" }),
      rec({ wrap: "c2", machine: M1, state: "unchanged" }),
      rec({ wrap: "c3", machine: M2, state: "applied" }),
      rec({ wrap: "c4", machine: M2, state: "sent" }),
      rec({ wrap: "other", machine: M1, state: "failed" }),
    ];
    expect(ruleAppliedMachines(["c1", "c2", "c3", "c4"], records)).toEqual([M1]);
    expect(ruleAppliedMachines(["c1"], records)).toEqual([M1]);
    expect(ruleAppliedMachines(undefined, records)).toEqual([]);
    expect(ruleAppliedMachines(["nope"], records)).toEqual([]);
  });

  it("appends '— applied on {device}', empty when nothing was acked", () => {
    expect(appliedSuffix([])).toBe("");
    expect(appliedSuffix(["Robin's laptop"])).toBe(" — applied on Robin's laptop");
    expect(appliedSuffix(["A", "B", "A"])).toBe(" — applied on A and B");
    expect(appliedSuffix(["A", "B", "C"])).toBe(" — applied on A, B and C");
  });
});
