import { describe, it, expect, vi, afterEach } from "vitest";
import type { DeviceStatus } from "../wire/status";
import { mergeDeviceStatus } from "./deviceStatusAdmit";

function feed(over: Partial<DeviceStatus>): DeviceStatus {
  return {
    v: 1,
    subject: "cd".repeat(32),
    machine: "ab".repeat(32),
    ts: 1000,
    dayKey: "2026-09-22",
    usedTodaySecs: 0,
    windowLeftSecs: 0,
    quotaLeftSecs: 0,
    effectiveSecs: 1800,
    locked: false,
    source: "guardian",
    ...over,
  };
}

describe("mergeDeviceStatus (review 2026-09-27 second round, F1/F2: newest ts wins unless the ward vouches a step back)", () => {
  it("stores the first-ever reading for a machine untouched", () => {
    const merged = mergeDeviceStatus(undefined, feed({ ts: 1000 }));
    expect(merged.ts).toBe(1000);
    expect(merged.clockWentBackwards).toBeUndefined();
  });

  it("accepts an ordinary forward-clocked update, exactly as before", () => {
    const cur = feed({ ts: 1000, usedTodaySecs: 30 });
    const merged = mergeDeviceStatus(cur, feed({ ts: 1060, usedTodaySecs: 90 }));
    expect(merged.ts).toBe(1060);
    expect(merged.usedTodaySecs).toBe(90);
    expect(merged.clockWentBackwards).toBeUndefined();
  });

  it("a duplicate delivery at the SAME ts as what's stored is a no-op, returned by reference", () => {
    const cur = feed({ ts: 1060, usedTodaySecs: 90 });
    const same = feed({ ts: 1060, usedTodaySecs: 90 });
    expect(mergeDeviceStatus(cur, same)).toBe(cur);
    // Even a DIFFERENT payload at the same ts is a no-op — the pre-batch
    // guard (`cur.ts >= status.ts ? prev : status`) ignored equal ts
    // unconditionally, and this restores exactly that.
    const differentSameTs = feed({ ts: 1060, usedTodaySecs: 999 });
    expect(mergeDeviceStatus(cur, differentSameTs)).toBe(cur);
  });

  // F1 (CONFIRMED by the review): a session's first sweep reads the whole
  // 24h window and relays do not serve it chronologically. Newest-ts-wins
  // must NOT regress to an older heartbeat that simply arrived later in the
  // batch — the old bug this restores the guard to prevent.
  it("F1 regression: a session-start sweep delivered newest-first must not let an older, forward-clocked heartbeat become current", () => {
    const newest = feed({ ts: 5000, usedTodaySecs: 500, pausedByAdmin: false });
    let stored: ReturnType<typeof mergeDeviceStatus> | undefined = mergeDeviceStatus(
      undefined,
      newest,
    );
    // The rest of the day's heartbeats arrive after it, out of order.
    for (const ts of [4000, 3000, 2000, 1000]) {
      stored = mergeDeviceStatus(stored, feed({ ts, usedTodaySecs: ts / 10 }));
    }
    expect(stored?.ts).toBe(5000);
    expect(stored?.usedTodaySecs).toBe(500);
    expect(stored?.clockWentBackwards).toBeUndefined();
  });

  // F2 (CONFIRMED by the review): nothing but the ±2-day wrap jitter bounds a
  // replay, so any relay can re-serve a genuine, but old, wrap. Without a
  // ward-vouched marker for THIS step, that must never displace current
  // state, however plausible the ts looks.
  it("F2 regression: a replayed old STATUS (no clockSteppedBackFrom) cannot become current", () => {
    const cur = feed({ ts: 1_000_000, usedTodaySecs: 500 });
    const replay = feed({ ts: 900_000, usedTodaySecs: 12 }); // genuine wrap, just old
    const merged = mergeDeviceStatus(cur, replay);
    expect(merged).toBe(cur);
    expect(merged.clockWentBackwards).toBeUndefined();
  });

  // F2 continued: a STALE marker — one that no longer reaches the currently
  // stored ts — must not be trusted either. Only a marker that is >= cur.ts
  // is proof this particular step-back is the one that produced THIS cur.
  it("a clockSteppedBackFrom that doesn't reach the stored ts is treated as a plain replay", () => {
    const cur = feed({ ts: 1_000_000, usedTodaySecs: 500 });
    const staleMarker = feed({ ts: 900_000, usedTodaySecs: 12, clockSteppedBackFrom: 999_000 });
    const merged = mergeDeviceStatus(cur, staleMarker);
    expect(merged).toBe(cur);
  });

  // The genuine case the wire field exists for: the ward's own marker,
  // vouching for THIS lower ts as a real step back from exactly cur.ts.
  it("admits and flags a genuine ward-vouched clock step (clockSteppedBackFrom === cur.ts)", () => {
    const cur = feed({ ts: 1_000_000, usedTodaySecs: 500 });
    const back = feed({ ts: 1_000_000 - 7 * 86_400, usedTodaySecs: 12, clockSteppedBackFrom: 1_000_000 });
    const merged = mergeDeviceStatus(cur, back);
    expect(merged.ts).toBe(back.ts);
    expect(merged.usedTodaySecs).toBe(12);
    expect(merged.clockWentBackwards).toBe(true);
  });

  it("also admits when clockSteppedBackFrom is strictly greater than cur.ts", () => {
    const cur = feed({ ts: 1000 });
    const back = feed({ ts: 500, clockSteppedBackFrom: 1500 });
    const merged = mergeDeviceStatus(cur, back);
    expect(merged.clockWentBackwards).toBe(true);
  });

  it("carries the flag through ordinary forward-clocked heartbeats while still below the threshold", () => {
    const cur = feed({ ts: 1_000_000 });
    const back = mergeDeviceStatus(
      cur,
      feed({ ts: 900_000, usedTodaySecs: 12, clockSteppedBackFrom: 1_000_000 }),
    );
    expect(back.clockWentBackwards).toBe(true);
    // The ward's clock is ticking forward again, but still hasn't reached
    // the pre-step ts — the flag must stay up, or the card would flicker
    // "back to normal" while still reading a time in the past.
    const stillBehind = mergeDeviceStatus(back, feed({ ts: 950_000, usedTodaySecs: 20 }));
    expect(stillBehind.ts).toBe(950_000);
    expect(stillBehind.clockWentBackwards).toBe(true);
  });

  it("clears the flag once a forward ts reaches or passes clockSteppedBackFrom", () => {
    const cur = feed({ ts: 1_000_000 });
    const back = mergeDeviceStatus(
      cur,
      feed({ ts: 900_000, clockSteppedBackFrom: 1_000_000 }),
    );
    const notYet = mergeDeviceStatus(back, feed({ ts: 999_999 }));
    expect(notYet.clockWentBackwards).toBe(true);
    const caughtUp = mergeDeviceStatus(notYet, feed({ ts: 1_000_000 }));
    expect(caughtUp.clockWentBackwards).toBeUndefined();
    expect(caughtUp.ts).toBe(1_000_000);
  });

  // The chosen clearing rule's other half: bound the flag's lifetime even if
  // the clock never visibly catches up (corrected to a still-earlier time,
  // then drifts up slowly, or the device is simply never heard from at that
  // vintage again).
  it("clears the flag after 24h even if the ward's clock never catches back up", () => {
    const flaggedAtMs = 10_000_000;
    const cur = feed({ ts: 1_000_000 });
    const back = mergeDeviceStatus(
      cur,
      feed({ ts: 900_000, clockSteppedBackFrom: 1_000_000 }),
      flaggedAtMs,
    );
    expect(back.clockWentBackwards).toBe(true);
    const justUnder = mergeDeviceStatus(
      back,
      feed({ ts: 900_060 }),
      flaggedAtMs + 24 * 3600 * 1000 - 1,
    );
    expect(justUnder.clockWentBackwards).toBe(true);
    const after = mergeDeviceStatus(back, feed({ ts: 900_120 }), flaggedAtMs + 24 * 3600 * 1000);
    expect(after.clockWentBackwards).toBeUndefined();
  });

  // An older ward (pre-marker) never sends `clockSteppedBackFrom` at all, so
  // every one of its lower-ts deliveries is an ordinary replay/out-of-order
  // case and is ignored exactly as it always was — the field is purely
  // additive.
  it("an older ward's STATUS (no clockSteppedBackFrom, ever) behaves exactly as pre-batch: newest ts wins, nothing else does", () => {
    let stored: ReturnType<typeof mergeDeviceStatus> | undefined;
    for (const ts of [1000, 1060, 1130, 1190]) {
      stored = mergeDeviceStatus(stored, feed({ ts }));
    }
    expect(stored?.ts).toBe(1190);
    expect(stored?.clockWentBackwards).toBeUndefined();
    // A late-arriving OLDER heartbeat from the same old ward is ignored.
    const afterOldReplay = mergeDeviceStatus(stored, feed({ ts: 1090 }));
    expect(afterOldReplay).toBe(stored);
  });
});

describe("mergeDeviceStatus — seq (review round 2, R2-2/R2-3: a monotonic per-device seq closes the replay hole ts+marker left open)", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("orders by seq alone when both sides carry it, ignoring ts entirely", () => {
    const cur = feed({ ts: 1_000_000, seq: 10, usedTodaySecs: 5 });
    // A forged-looking LOWER ts but a genuinely HIGHER seq still wins.
    const merged = mergeDeviceStatus(cur, feed({ ts: 1, seq: 11, usedTodaySecs: 9 }));
    expect(merged.seq).toBe(11);
    expect(merged.usedTodaySecs).toBe(9);
  });

  it("an equal seq is a duplicate re-delivery, ignored by reference", () => {
    const cur = feed({ ts: 1_000_000, seq: 10 });
    const dup = feed({ ts: 1_000_000, seq: 10, usedTodaySecs: 999 });
    expect(mergeDeviceStatus(cur, dup)).toBe(cur);
  });

  // R2-2's exact regression: the marker's size is ward-controlled and
  // reusable, so an old, genuine step-back STATUS could be re-published
  // (the gift wrap is public on the relays) and, on ts+marker alone, out-
  // rank a newer reading for as long as the marker still reached it. Once
  // both sides carry seq, a replay's seq can never be newer than what it
  // already lost to.
  it("R2-2: a replayed old step-back STATUS, however large its clockSteppedBackFrom, cannot become current once seq is present", () => {
    const cur = feed({ ts: 1_000_000, seq: 42, usedTodaySecs: 500 });
    const replayedStepBack = feed({
      ts: 100,
      seq: 5, // the replay's OWN, older seq — genuinely signed, just stale.
      usedTodaySecs: 1,
      clockSteppedBackFrom: 999_999_999, // absurdly large; would have won on ts+marker alone.
    });
    const merged = mergeDeviceStatus(cur, replayedStepBack);
    expect(merged).toBe(cur);
  });

  // R2-3's exact regression: a restart drops the ward's in-memory ts marker,
  // freezing the guardian's view until real time passes the wrapped-to ts.
  // A persisted seq survives the restart and is not frozen by it.
  it("R2-3: a reboot (seq continues, ts lower after an RTC correction) is admitted, and flags the backward step", () => {
    const cur = feed({ ts: 1_000_000, seq: 100, usedTodaySecs: 500 });
    const postReboot = feed({ ts: 999_000, seq: 101, usedTodaySecs: 501 });
    const merged = mergeDeviceStatus(cur, postReboot);
    expect(merged.seq).toBe(101);
    expect(merged.ts).toBe(999_000);
    expect(merged.clockWentBackwards).toBe(true);
  });

  it("sets the backward-clock notice on an admitted seq-ordered status that carries clockSteppedBackFrom, even on a forward ts", () => {
    const cur = feed({ ts: 1_000_000, seq: 10 });
    const merged = mergeDeviceStatus(
      cur,
      feed({ ts: 1_000_100, seq: 11, clockSteppedBackFrom: 999_000 }),
    );
    expect(merged.clockWentBackwards).toBe(true);
  });

  it("an ordinary forward, seq-ordered status with no marker does not flag the notice", () => {
    const cur = feed({ ts: 1_000_000, seq: 10 });
    const merged = mergeDeviceStatus(cur, feed({ ts: 1_000_100, seq: 11 }));
    expect(merged.clockWentBackwards).toBeUndefined();
  });

  it("still clears a carried notice via the existing rule once a seq-ordered forward ts catches up", () => {
    const cur = feed({ ts: 1_000_000, seq: 10 });
    const back = mergeDeviceStatus(cur, feed({ ts: 999_000, seq: 11, clockSteppedBackFrom: 1_000_000 }));
    expect(back.clockWentBackwards).toBe(true);
    const caughtUp = mergeDeviceStatus(back, feed({ ts: 1_000_000, seq: 12 }));
    expect(caughtUp.clockWentBackwards).toBeUndefined();
  });

  describe("the upgrade transition: incoming carries seq, stored does not", () => {
    it("admits outright on a forward or equal ts", () => {
      const cur = feed({ ts: 1_000_000 });
      const merged = mergeDeviceStatus(cur, feed({ ts: 1_000_000, seq: 1, usedTodaySecs: 7 }));
      expect(merged.seq).toBe(1);
      expect(merged.usedTodaySecs).toBe(7);
      expect(merged.clockWentBackwards).toBeUndefined();
    });

    it("admits a ts within 5 minutes behind stored, and flags the backward step", () => {
      const cur = feed({ ts: 1_000_000 });
      const merged = mergeDeviceStatus(cur, feed({ ts: 1_000_000 - 200, seq: 1 }));
      expect(merged.seq).toBe(1);
      expect(merged.ts).toBe(1_000_000 - 200);
      expect(merged.clockWentBackwards).toBe(true);
    });

    it("rejects a ts more than 5 minutes behind stored with no vouched marker — a replay from before the upgrade", () => {
      const cur = feed({ ts: 1_000_000 });
      const merged = mergeDeviceStatus(cur, feed({ ts: 1_000_000 - 400, seq: 1 }));
      expect(merged).toBe(cur);
    });

    it("still admits beyond the 5-minute tolerance when the ward-vouched marker reaches stored ts", () => {
      const cur = feed({ ts: 1_000_000 });
      const merged = mergeDeviceStatus(
        cur,
        feed({ ts: 1_000_000 - 7 * 86_400, seq: 1, clockSteppedBackFrom: 1_000_000 }),
      );
      expect(merged.seq).toBe(1);
      expect(merged.clockWentBackwards).toBe(true);
    });
  });

  it("R2-2/R2-3 continued: incoming lacks seq while stored has one — never current, and logs once", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const cur = feed({ ts: 1_000_000, seq: 10, usedTodaySecs: 500 });
    // A much newer-looking ts, no seq at all — a replay from before the
    // upgrade, or a downgraded ward. Must not win merely on ts.
    const merged = mergeDeviceStatus(cur, feed({ ts: 9_999_999, usedTodaySecs: 999 }));
    expect(merged).toBe(cur);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  // The scenario the fix is FOR: a session-start sweep of the day's wraps
  // arrives newest-first (as F1 already required), interleaved with an
  // attempted replay of an earlier, lower-seq reading. The final stored
  // status must never regress below the highest seq actually delivered,
  // regardless of arrival order or of what ts a replay claims.
  it("session-start newest-first fetch: the highest seq wins regardless of arrival order, and never regresses", () => {
    let stored: ReturnType<typeof mergeDeviceStatus> | undefined;
    const batch = [
      feed({ ts: 5000, seq: 50, usedTodaySecs: 500 }),
      feed({ ts: 4000, seq: 40, usedTodaySecs: 400 }),
      // A replay of an old wrap with a huge ts/marker, but its real, stale seq.
      feed({ ts: 999_999, seq: 10, clockSteppedBackFrom: 999_998, usedTodaySecs: 1 }),
      feed({ ts: 3000, seq: 30, usedTodaySecs: 300 }),
    ];
    for (const s of batch) {
      stored = mergeDeviceStatus(stored, s);
    }
    expect(stored?.seq).toBe(50);
    expect(stored?.ts).toBe(5000);
    expect(stored?.usedTodaySecs).toBe(500);
  });
});
