import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import PresenceScanner from "./PresenceScanner";
import { presenceCodeFrom } from "./guardiansView";
import {
  LAPTOP,
  OTHER,
  STATEMENT,
  TOKEN,
  button,
  device,
  family,
  guardians,
  mockStore,
  signetRoot,
  typeInto,
} from "./guardiansTestKit";

const store = vi.hoisted(() => ({ current: null as unknown }));
vi.mock("../store/store", () => ({ useCharter: () => store.current }));
// The camera isn't available in jsdom: capture what the screen does with a scan.
const scanner = vi.hoisted(() => ({ onScan: null as ((data: string) => boolean) | null }));
vi.mock("../components/QRScanner", () => ({
  QRScanner: ({ onScan }: { onScan: (data: string) => boolean }) => {
    scanner.onScan = onScan;
    return null;
  },
}));

const removal = `kintrinsic/unguardian/${OTHER}`;
const valid = presenceCodeFrom(LAPTOP, TOKEN, removal, STATEMENT);

describe("presence scanner", () => {
  let container: HTMLDivElement;
  let root: Root;
  let s: ReturnType<typeof mockStore>;

  beforeEach(() => {
    const g = guardians({ pending: [{ d: removal, kind: "removal", takesEffectAt: 5, expiresAt: 9_999_999_999 }] });
    s = mockStore({
      children: family([device("Laptop", LAPTOP, "linux", g)]),
      keys: { root: signetRoot(), sent: { [removal]: { id: STATEMENT, type: "removal", issuedAt: 9_999_999_999 - 7 * 86400, key: OTHER } } },
    });
    store.current = s;
    scanner.onScan = null;
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
    act(() => root.render(<PresenceScanner />));
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("explains where each kind of ward shows its code", () => {
    expect(container.textContent).toContain("Kintrinsic → Connect a phone → Confirm in person → Show the code to confirm");
    expect(container.textContent).toContain("About this device → Guardians → Guardian presence");
    expect(container.textContent).toContain("guardian PIN");
  });

  it("accepts a scanned presence code and confirms it", async () => {
    await act(async () => button(container, "Scan the code")!.click());
    let taken = false;
    await act(async () => {
      taken = scanner.onScan!(valid);
    });
    expect(taken).toBe(true);
    expect(s.guardianKeys.confirmPresence).toHaveBeenCalledWith(valid);
    expect(container.textContent).toContain("Confirmation sent");
    expect(container.textContent).toContain("Sent to Laptop.");
  });

  it("keeps scanning past a QR that isn't a presence code", async () => {
    await act(async () => button(container, "Scan the code")!.click());
    expect(scanner.onScan!("https://example.com")).toBe(false);
    expect(scanner.onScan!(valid.replace(`t=${TOKEN}`, "t=XYZ"))).toBe(false);
    expect(s.guardianKeys.confirmPresence).not.toHaveBeenCalled();
  });

  it("rejects a typed code that isn't one", async () => {
    act(() => typeInto(container.querySelector("input")!, "hello"));
    await act(async () => button(container, "Use this code")!.click());
    expect(container.querySelector('[role="alert"]')!.textContent).toContain("isn't a Kintrinsic presence code");
    expect(s.guardianKeys.confirmPresence).not.toHaveBeenCalled();
  });

  it("shows the ward's refusal when the confirmation can't be sent", async () => {
    s.guardianKeys.confirmPresence.mockRejectedValueOnce(new Error("Couldn't reach any relay — check your connection and try again."));
    act(() => typeInto(container.querySelector("input")!, valid));
    await act(async () => button(container, "Use this code")!.click());
    expect(container.querySelector('[role="alert"]')!.textContent).toContain("Couldn't reach any relay");
  });

  it("takes the short form, then the device and what it confirms", async () => {
    act(() => typeInto(container.querySelector("input")!, "DDDDDDDD dddddddd-dddddddd dddddddd"));
    await act(async () => button(container, "Use this code")!.click());
    const [deviceSelect] = [...container.querySelectorAll("select")];
    act(() => typeInto(deviceSelect, LAPTOP));
    const itemSelect = [...container.querySelectorAll("select")][1];
    act(() => typeInto(itemSelect, removal));
    await act(async () => button(container, "Confirm in person")!.click());
    expect(s.guardianKeys.confirmPresence).toHaveBeenCalledWith(valid);
  });
});
