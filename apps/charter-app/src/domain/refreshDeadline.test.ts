import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { raceRefresh } from "./refreshDeadline";

describe("raceRefresh", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("reports done when the work finishes inside the deadline", async () => {
    const outcome = raceRefresh(Promise.resolve("data"), 10_000);
    await expect(outcome).resolves.toBe("done");
  });

  it("reports failed when the work rejects — and never throws", async () => {
    const outcome = raceRefresh(Promise.reject(new Error("relay refused")), 10_000);
    await expect(outcome).resolves.toBe("failed");
  });

  it("reports timeout when the work dangles past the deadline", async () => {
    const never = new Promise(() => {});
    const outcome = raceRefresh(never, 10_000);
    await vi.advanceTimersByTimeAsync(10_000);
    await expect(outcome).resolves.toBe("timeout");
  });

  it("a slow success still counts as done just under the wire", async () => {
    const slow = new Promise((r) => setTimeout(r, 9_999));
    const outcome = raceRefresh(slow, 10_000);
    await vi.advanceTimersByTimeAsync(9_999);
    await expect(outcome).resolves.toBe("done");
  });
});
