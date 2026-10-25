import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Child, Device } from "../domain/types";
import DeviceRelays from "./DeviceRelays";

const actions = vi.hoisted(() => ({ createRelayCode: vi.fn(), recordDeviceRelays: vi.fn() }));
vi.mock("../store/store", () => ({ useCharter: () => actions }));

const relays = ["wss://one.example", "wss://two.example"];
const device: Device = { id: "d", label: "Robin’s phone", platform: "android", pairing: "paired", devicePubkey: "a".repeat(64), relays };
const child: Child = { id: "c", name: "Robin", color: "#abc", dependantPubkey: null, devices: [device], policies: [] };

describe("device relay recovery", () => {
  let container: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    vi.clearAllMocks();
    container = document.createElement("div");
    document.body.append(container);
    act(() => {
      root = createRoot(container);
      root.render(<DeviceRelays child={child} device={device} />);
    });
  });
  afterEach(() => { act(() => root.unmount()); container.remove(); });

  async function click(label: string) {
    const button = [...container.querySelectorAll("button")].find((b) => b.textContent === label);
    expect(button, label).toBeDefined();
    await act(async () => { button!.click(); });
  }

  it("keeps the old delivery list until the guardian confirms application on the ward", async () => {
    const nextRelays = ["wss://new.example", "wss://other.example"];
    const code = JSON.stringify({ created_at: 123, content: JSON.stringify({ machine: device.devicePubkey, relays: nextRelays }) });
    actions.createRelayCode.mockResolvedValue(code);
    await click("Relays");
    const input = container.querySelector("textarea")!;
    act(() => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, nextRelays.join("\n"));
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await click("Create relay code");
    expect(actions.createRelayCode).toHaveBeenCalledWith(child.id, device.id, nextRelays);
    expect(actions.recordDeviceRelays).not.toHaveBeenCalled();
    await click("Relays");
    await click("Relays");
    expect(container.querySelector("textarea")!.value).toBe(nextRelays.join("\n"));
    expect(container.textContent).toContain("About this device → Change relays");
    await click("The device applied this — save relays");
    expect(actions.recordDeviceRelays).toHaveBeenCalledWith(child.id, device.id, nextRelays, 123);
  });

  it("reports signing failures without recording a relay change", async () => {
    actions.createRelayCode.mockRejectedValue(new Error("Signing was cancelled."));
    await click("Relays");
    await click("Create relay code");
    expect(container.querySelector('[role="alert"]')?.textContent).toBe("Signing was cancelled.");
    expect(actions.recordDeviceRelays).not.toHaveBeenCalled();
  });
});
