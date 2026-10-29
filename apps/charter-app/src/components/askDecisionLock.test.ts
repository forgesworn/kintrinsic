import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ASK_DECISION_STALE_MS,
  endAskDecision,
  tryBeginAskDecision,
} from "./askDecisionLock";

describe("askDecisionLock", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("allows one holder at a time, and frees on end", () => {
    const a = tryBeginAskDecision();
    expect(a).not.toBeNull();
    expect(tryBeginAskDecision()).toBeNull();
    endAskDecision(a!);
    expect(tryBeginAskDecision()).not.toBeNull();
  });

  it("a hold that never settles goes stale and the lock can be taken again", () => {
    expect(tryBeginAskDecision()).not.toBeNull();
    vi.advanceTimersByTime(ASK_DECISION_STALE_MS - 1);
    expect(tryBeginAskDecision()).toBeNull();
    vi.advanceTimersByTime(1);
    expect(tryBeginAskDecision()).not.toBeNull();
  });

  it("the stale holder finishing late does not release a newer holder's lock", () => {
    const stale = tryBeginAskDecision()!;
    vi.advanceTimersByTime(ASK_DECISION_STALE_MS);
    const fresh = tryBeginAskDecision()!;
    expect(fresh).not.toBeNull();
    endAskDecision(stale);
    expect(tryBeginAskDecision()).toBeNull();
    endAskDecision(fresh);
    expect(tryBeginAskDecision()).not.toBeNull();
  });
});
