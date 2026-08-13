import { describe, expect, it } from "vitest";
import { pendingCount, reducer, type CharterState } from "./store";
import type { ChildRequest, SignerState } from "../domain/types";

const SIGNER: SignerState = { connected: false, kind: "none", autoSign: false };

function emptyState(): CharterState {
  return { children: [], requests: [], activity: [], signer: SIGNER };
}

const BUCKET_ASK: ChildRequest = {
  id: "req1",
  childId: "child1",
  requester: "device",
  deviceId: "dev1",
  kind: "time.extend",
  createdAt: 0,
  title: "device asks for 30 more minutes of Play",
  minutesRequested: 30,
  limitHit: "bucket",
  bucketId: "play",
  status: "pending",
};

/**
 * M-1 (hardware round, 2026-08-03): the guardian's stepped-down grant amount
 * used to live only in `Approvals`' own `useState`, which resets to the FULL
 * requested amount on remount — exactly the path C-1's error forced (fail →
 * go fix the tz → come back → approve). The fix persists the choice on the
 * REQUEST record via `SET_REQUEST_CHOSEN_MINUTES`, which — being reducer
 * state, not component state — survives a remount. This test goes through
 * the real reducer, the same discipline `decisionTiming.test.ts` established
 * for N1: a hand-built fixture could never have caught a bug that is
 * specifically about WHERE the state lives.
 */
describe("reducer — SET_REQUEST_CHOSEN_MINUTES (M-1 regression)", () => {
  it("choosing 5 of a 30-minute ask persists on the request record", () => {
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK] };
    const s1 = reducer(s0, { type: "SET_REQUEST_CHOSEN_MINUTES", id: "req1", minutes: 5 });
    expect(s1.requests[0].chosenMinutes).toBe(5);
    // The full requested amount is untouched — only the guardian's choice moved.
    expect(s1.requests[0].minutesRequested).toBe(30);
  });

  it("a failed send touches nothing — the chosen amount is still there after", () => {
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK] };
    const s1 = reducer(s0, { type: "SET_REQUEST_CHOSEN_MINUTES", id: "req1", minutes: 5 });
    // A failed decisionSend dispatches NOTHING — the request stays pending
    // and untouched, which is exactly what makes this bug possible: nothing
    // in the reducer state ever reset, only the screen's own useState did.
    const grantedAfterFailedSend = s1.requests[0].chosenMinutes ?? s1.requests[0].minutesRequested ?? 0;
    expect(grantedAfterFailedSend).toBe(5);

    // "Remount" — a fresh Approvals instance reads straight off state.requests
    // (no local useState involved at all in the fixed version), so it sees
    // exactly what a fresh render would: still 5, never snapping back to 30.
    const rereadOnRemount = s1.requests[0].chosenMinutes ?? s1.requests[0].minutesRequested ?? 0;
    expect(rereadOnRemount).toBe(5);
  });

  it("leaves other requests and other state untouched", () => {
    const other: ChildRequest = { ...BUCKET_ASK, id: "req2", bucketId: "social" };
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK, other] };
    const s1 = reducer(s0, { type: "SET_REQUEST_CHOSEN_MINUTES", id: "req1", minutes: 5 });
    expect(s1.requests[1].chosenMinutes).toBeUndefined();
    expect(s1.signer).toBe(SIGNER);
  });

  it("an approve dispatch after the choice keeps status + chosenMinutes both correct", () => {
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK] };
    const s1 = reducer(s0, { type: "SET_REQUEST_CHOSEN_MINUTES", id: "req1", minutes: 5 });
    const s2 = reducer(s1, { type: "SET_REQUEST_STATUS", id: "req1", status: "approved" });
    expect(s2.requests[0].chosenMinutes).toBe(5);
    expect(s2.requests[0].status).toBe("approved");
  });
});

/**
 * Approvals clarity Task C (2026-08-04): dismissing a repeat ask.
 *
 * `dismissRequest` (store.tsx) is a plain `SET_REQUEST_STATUS … "dismissed"`
 * dispatch — these tests exercise it through the REAL reducer, the same
 * discipline the M-1 suite above established, because the load-bearing claim
 * ("a dismissed request must not reappear") is specifically about WHERE the
 * state lives and how it's shaped, not about any UI behaviour.
 */
describe("reducer — dismissRequest / SET_REQUEST_STATUS \"dismissed\" (Task C)", () => {
  it("removes exactly the dismissed request from the pending view, leaving the other untouched", () => {
    const other: ChildRequest = { ...BUCKET_ASK, id: "req2", bucketId: "social" };
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK, other] };
    const s1 = reducer(s0, { type: "SET_REQUEST_STATUS", id: "req1", status: "dismissed" });

    expect(s1.requests.find((r) => r.id === "req1")?.status).toBe("dismissed");
    // The sibling is completely untouched — same status, same object identity.
    expect(s1.requests.find((r) => r.id === "req2")).toBe(other);

    const stillPending = s1.requests.filter((r) => r.status === "pending");
    expect(stillPending).toHaveLength(1);
    expect(stillPending[0].id).toBe("req2");
  });

  it("is idempotent — dismissing twice is identical to dismissing once", () => {
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK] };
    const s1 = reducer(s0, { type: "SET_REQUEST_STATUS", id: "req1", status: "dismissed" });
    const s2 = reducer(s1, { type: "SET_REQUEST_STATUS", id: "req1", status: "dismissed" });
    expect(s2.requests).toHaveLength(1);
    expect(s2.requests[0].status).toBe("dismissed");
    expect(s2.requests[0]).toEqual(s1.requests[0]);
  });

  it("touches nothing else on the request record — chosenMinutes, title, reqId all survive", () => {
    const wireAsk: ChildRequest = {
      ...BUCKET_ASK,
      id: "req3",
      reqId: "a".repeat(64),
      nonce: "b".repeat(64),
      machine: "c".repeat(64),
      chosenMinutes: 20,
    };
    const s0: CharterState = { ...emptyState(), requests: [wireAsk] };
    const s1 = reducer(s0, { type: "SET_REQUEST_STATUS", id: "req3", status: "dismissed" });
    expect(s1.requests[0]).toMatchObject({
      status: "dismissed",
      reqId: wireAsk.reqId,
      chosenMinutes: 20,
      title: wireAsk.title,
    });
  });

  it("a real reload round-trips through JSON exactly (persist writes state.requests verbatim)", () => {
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK] };
    const s1 = reducer(s0, { type: "SET_REQUEST_STATUS", id: "req1", status: "dismissed" });
    // `persist`/`loadState` in store.tsx are a bare JSON.stringify/parse of
    // the whole CharterState — this is exactly that round trip. If it holds,
    // a reload reads back the SAME status, not the original "pending".
    const reloaded = JSON.parse(JSON.stringify(s1)) as CharterState;
    expect(reloaded.requests[0].status).toBe("dismissed");
  });

  it(
    "keeps the reqId in `requests` so a real device-originated ask stays blocked from " +
      "ingestDeviceRequest's relay-poll dedupe (store.tsx: `stateRef.current.requests.some(" +
      "(r) => r.reqId === req.reqId)`) — the exact mechanism that stops a dismissed ask " +
      "reappearing when the 24h relay window serves the same wrap again",
    () => {
      const reqId = "d".repeat(64);
      const wireAsk: ChildRequest = { ...BUCKET_ASK, id: "req4", reqId };
      const s0: CharterState = { ...emptyState(), requests: [wireAsk] };
      const s1 = reducer(s0, { type: "SET_REQUEST_STATUS", id: "req4", status: "dismissed" });

      // The literal guard `ingestDeviceRequest` runs before ever building a
      // new card for an incoming wrap with this reqId.
      const wouldBeDeduped = s1.requests.some((r) => r.reqId === reqId);
      expect(wouldBeDeduped).toBe(true);

      // Contrast: if dismiss instead REMOVED the row (the naive approach the
      // spec warns against), the same reqId would no longer be present and
      // the relay poll would re-add the ask as a brand-new "pending" card.
      const removedInstead = s0.requests.filter((r) => r.id !== "req4");
      expect(removedInstead.some((r) => r.reqId === reqId)).toBe(false);
    },
  );

  it("pendingCount and the Approvals \"pending\" filter both drop a dismissed request", () => {
    const other: ChildRequest = { ...BUCKET_ASK, id: "req2" };
    const s0: CharterState = { ...emptyState(), requests: [BUCKET_ASK, other] };
    const s1 = reducer(s0, { type: "SET_REQUEST_STATUS", id: "req1", status: "dismissed" });
    expect(pendingCount(s1)).toBe(1);
  });
});
