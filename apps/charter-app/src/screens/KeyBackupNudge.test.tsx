import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import KeyBackupNudge, { KEY_BACKUP_ANCHOR } from "./KeyBackupNudge";
import { markKeyBackedUp } from "../signer/backupStatus";

const PK = "c".repeat(64);
const hoisted = vi.hoisted(() => ({
  signer: { connected: true, kind: "local" } as { connected: boolean; kind: string },
}));
vi.mock("../store/store", () => ({ useCharter: () => ({ state: { signer: hoisted.signer } }) }));
vi.mock("../signer/guardianKey", () => ({ guardianPubkeyHex: () => PK }));

const TEXT =
  "Save a backup of your parent key — without it, a lost phone means setting every device up again.";

describe("key backup reminder", () => {
  let container: HTMLDivElement;
  let root: Root;

  function render() {
    act(() => {
      root = createRoot(container);
      root.render(<KeyBackupNudge />);
    });
  }
  const button = (label: string) =>
    [...container.querySelectorAll("button")].find((b) => b.textContent === label);

  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    hoisted.signer = { connected: true, kind: "local" };
    container = document.createElement("div");
    document.body.append(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("shows for a local key with no backup", () => {
    render();
    expect(container.textContent).toContain(TEXT);
  });

  it("stays away when a backup of this key exists", () => {
    markKeyBackedUp(PK);
    render();
    expect(container.textContent).not.toContain("Save a backup");
  });

  it("returns if the key being backed up is a different one", () => {
    markKeyBackedUp("d".repeat(64));
    render();
    expect(container.textContent).toContain("Save a backup");
  });

  it("is not shown when approval is off, or held by Signet", () => {
    hoisted.signer = { connected: false, kind: "none" };
    render();
    expect(container.textContent).toBe("");
    act(() => root.unmount());
    hoisted.signer = { connected: true, kind: "signet" };
    render();
    expect(container.textContent).toBe("");
  });

  it("'Not now' hides it for this session only", () => {
    render();
    act(() => button("Not now")!.click());
    expect(container.textContent).toBe("");
    // A new session (tab) asks again.
    act(() => root.unmount());
    sessionStorage.clear();
    render();
    expect(container.textContent).toContain("Save a backup");
  });

  it("'Back up now' scrolls to the existing backup section", () => {
    const target = document.createElement("div");
    target.id = KEY_BACKUP_ANCHOR;
    const scroll = vi.fn();
    target.scrollIntoView = scroll;
    document.body.append(target);
    render();
    act(() => button("Back up now")!.click());
    expect(scroll).toHaveBeenCalledTimes(1);
    target.remove();
  });

  it("disappears as soon as a backup is exported", () => {
    render();
    expect(container.textContent).toContain("Save a backup");
    act(() => markKeyBackedUp(PK));
    expect(container.textContent).toBe("");
  });
});
