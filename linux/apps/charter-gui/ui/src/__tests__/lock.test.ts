import { describe, it, expect } from "vitest";
import { LockState } from "../lock";

describe("lock", () => {
  it("no web action can dismiss the lock", () => {
    const lock = new LockState();
    lock.onLockStateChanged(true, "bedtime");
    expect(lock.isLocked()).toBe(true);

    // Every web attempt is a no-op.
    expect(lock.requestDismissFromWeb()).toBe(false);
    expect(lock.requestDismissFromWeb()).toBe(false);
    expect(lock.isLocked()).toBe(true);
    expect(lock.currentReason()).toBe("bedtime");
  });

  it("only the daemon LockStateChanged{false} unlocks", () => {
    const lock = new LockState();
    lock.onLockStateChanged(true, "budget");
    expect(lock.isLocked()).toBe(true);
    lock.onLockStateChanged(false);
    expect(lock.isLocked()).toBe(false);
    expect(lock.currentReason()).toBeUndefined();
  });
});
