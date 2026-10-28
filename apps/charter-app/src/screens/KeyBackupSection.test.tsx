import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import KeyBackupSection from "./KeyBackupSection";
import { isKeyBackedUp } from "../signer/backupStatus";

const PK = "e".repeat(64);
const restoreGuardianKey = vi.hoisted(() => vi.fn());
vi.mock("../store/store", () => ({
  useCharter: () => ({ restoreGuardianKey }),
  exportPersistedState: () => "{}",
  importPersistedState: () => {},
}));
vi.mock("../signer/guardianKey", () => ({
  guardianPubkeyHex: () => PK,
  loadOrCreateGuardianKey: () => new Uint8Array(32),
}));
vi.mock("../signer/keyBackup", () => ({
  encryptGuardianBackup: async () => "CHARTER-KEYBAK.test",
  decryptGuardianBackup: async () => ({ secret: new Uint8Array(32), stateJson: null }),
}));

describe("key backup section records an export", () => {
  let container: HTMLDivElement;
  let root: Root;
  const button = (label: string) =>
    [...container.querySelectorAll("button")].find((b) => b.textContent === label);
  const type = (label: string, value: string) => {
    const el = container.querySelector(`[aria-label="${label}"]`) as HTMLInputElement | HTMLTextAreaElement;
    const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
    el.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const settle = () => act(async () => { await new Promise((r) => setTimeout(r, 0)); });

  beforeEach(async () => {
    localStorage.clear();
    sessionStorage.clear();
    restoreGuardianKey.mockResolvedValue(PK);
    container = document.createElement("div");
    document.body.append(container);
    act(() => {
      root = createRoot(container);
      root.render(<KeyBackupSection busy={false} />);
    });
    act(() => button("Back up key")!.click());
    act(() => {
      type("Backup passphrase", "correct horse");
      type("Repeat backup passphrase", "correct horse");
    });
    await act(async () => button("Create backup")!.click());
    await settle();
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.restoreAllMocks();
  });

  it("does not count merely making the backup", () => {
    expect(container.querySelector("[data-backup-blob]")).not.toBeNull();
    expect(isKeyBackedUp(PK)).toBe(false);
  });

  it("counts a copy", () => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: vi.fn() }, configurable: true });
    act(() => button("Copy")!.click());
    expect(isKeyBackedUp(PK)).toBe(true);
  });

  it("counts a download", () => {
    URL.createObjectURL = vi.fn(() => "blob:x");
    URL.revokeObjectURL = vi.fn();
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    act(() => button("Download .txt")!.click());
    expect(isKeyBackedUp(PK)).toBe(true);
  });
});
