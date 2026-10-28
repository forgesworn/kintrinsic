import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  BACKUP_STATUS_EVENT,
  dismissBackupNudge,
  isBackupNudgeDismissed,
  isKeyBackedUp,
  markKeyBackedUp,
} from "./backupStatus";

const PK = "a".repeat(64);
const OTHER = "b".repeat(64);

describe("backupStatus", () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
  });

  it("starts with no backup recorded", () => {
    expect(isKeyBackedUp(PK)).toBe(false);
  });

  it("remembers a backup for that key, and only that key", () => {
    markKeyBackedUp(PK);
    expect(isKeyBackedUp(PK)).toBe(true);
    // A different guardian key has not been backed up.
    expect(isKeyBackedUp(OTHER)).toBe(false);
  });

  it("survives a reload (localStorage) but a dismissal does not (sessionStorage)", () => {
    markKeyBackedUp(PK);
    dismissBackupNudge();
    expect(isBackupNudgeDismissed()).toBe(true);
    sessionStorage.clear(); // a new tab / session
    expect(isBackupNudgeDismissed()).toBe(false);
    expect(isKeyBackedUp(PK)).toBe(true);
  });

  it("announces changes so open screens can re-read", () => {
    const seen = vi.fn();
    window.addEventListener(BACKUP_STATUS_EVENT, seen);
    markKeyBackedUp(PK);
    dismissBackupNudge();
    window.removeEventListener(BACKUP_STATUS_EVENT, seen);
    expect(seen).toHaveBeenCalledTimes(2);
  });

  it("does not throw when storage is unavailable", () => {
    const get = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const set = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(isKeyBackedUp(PK)).toBe(false);
    expect(() => markKeyBackedUp(PK)).not.toThrow();
    expect(() => dismissBackupNudge()).not.toThrow();
    get.mockRestore();
    set.mockRestore();
  });
});
