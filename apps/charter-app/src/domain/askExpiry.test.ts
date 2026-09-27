import { describe, it, expect } from "vitest";
import { ASK_EXPIRY_SECS, isAskExpired } from "./askExpiry";

describe("isAskExpired (G-3: the ward's own 24h ask TTL, commit 7ba043b)", () => {
  const NOW = Date.parse("2026-09-27T12:00:00Z");

  it("is not expired well within the day", () => {
    expect(isAskExpired(NOW - 3600 * 1000, NOW)).toBe(false);
  });

  it("is not expired one second short of the TTL", () => {
    const createdAt = NOW - (ASK_EXPIRY_SECS * 1000 - 1000);
    expect(isAskExpired(createdAt, NOW)).toBe(false);
  });

  it("is expired exactly at the TTL — matches the device's own `>` boundary closely enough to never under-warn", () => {
    const createdAt = NOW - ASK_EXPIRY_SECS * 1000;
    expect(isAskExpired(createdAt, NOW)).toBe(true);
  });

  it("is expired well past the TTL (the 30-hour case from the review)", () => {
    const createdAt = NOW - 30 * 3600 * 1000;
    expect(isAskExpired(createdAt, NOW)).toBe(true);
  });

  it("ASK_EXPIRY_SECS is exactly 24h, matching charter-spine::broker::PENDING_TTL_SECS", () => {
    expect(ASK_EXPIRY_SECS).toBe(24 * 3600);
  });
});
