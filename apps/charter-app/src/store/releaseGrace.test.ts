import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  RELEASE_LINK_GRACE_MS,
  RELEASE_SEND_GRACE_MS,
  onReleaseGraceEnd,
  releaseLinkDeadlines,
  resetReleaseGraceForTests,
  settleReleaseGrace,
  startReleaseGrace,
} from "./releaseGrace";

const M = "a".repeat(64);

describe("the release link grace", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    resetReleaseGraceForTests();
    vi.useRealTimers();
  });

  it("holds a machine for 2 minutes from the click, while the send is still being approved", () => {
    expect(RELEASE_SEND_GRACE_MS).toBe(120_000);
    startReleaseGrace(M);
    expect(releaseLinkDeadlines().get(M)).toBe(Date.now() + 120_000);
    vi.advanceTimersByTime(119_999);
    expect(releaseLinkDeadlines().has(M)).toBe(true);
  });

  it("a slow approval does not cost the grace: settling after 60 s gives a full 10 s from then", () => {
    const changed = vi.fn();
    onReleaseGraceEnd(changed);
    const token = startReleaseGrace(M);
    vi.advanceTimersByTime(60_000);
    settleReleaseGrace(M, token);
    expect(RELEASE_LINK_GRACE_MS).toBe(10_000);
    expect(releaseLinkDeadlines().get(M)).toBe(Date.now() + 10_000);
    // The roster is pushed at once with the new deadline.
    expect(changed).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(9_999);
    expect(releaseLinkDeadlines().has(M)).toBe(true);
    expect(changed).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(1);
    expect(releaseLinkDeadlines().has(M)).toBe(false);
    // And again when it runs out, so the roster loses its linkUntil.
    expect(changed).toHaveBeenCalledTimes(2);
  });

  it("a settle never lengthens a grace that has run out", () => {
    const token = startReleaseGrace(M);
    vi.advanceTimersByTime(RELEASE_SEND_GRACE_MS + 1);
    const changed = vi.fn();
    onReleaseGraceEnd(changed);
    settleReleaseGrace(M, token);
    expect(releaseLinkDeadlines().has(M)).toBe(false);
    expect(changed).not.toHaveBeenCalled();
  });

  it("with no settle the 2 minutes run out and the listener is told once", () => {
    const end = vi.fn();
    onReleaseGraceEnd(end);
    startReleaseGrace(M);
    vi.advanceTimersByTime(RELEASE_SEND_GRACE_MS);
    expect(releaseLinkDeadlines().has(M)).toBe(false);
    expect(end).toHaveBeenCalledTimes(1);
  });

  it("a late answer to an earlier release cannot move a newer release's deadline", () => {
    const first = startReleaseGrace(M);
    vi.advanceTimersByTime(5_000);
    startReleaseGrace(M);
    const second = Date.now() + RELEASE_SEND_GRACE_MS;
    settleReleaseGrace(M, first);
    expect(releaseLinkDeadlines().get(M)).toBe(second);
  });

  it("reads as over at the deadline whatever the timer says", () => {
    startReleaseGrace(M, 1_000);
    expect(releaseLinkDeadlines(1_000 + RELEASE_SEND_GRACE_MS - 1).has(M)).toBe(true);
    expect(releaseLinkDeadlines(1_000 + RELEASE_SEND_GRACE_MS).has(M)).toBe(false);
  });
});
