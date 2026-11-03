import { afterEach, describe, expect, it, vi } from "vitest";
import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import BatteryControls from "./BatteryControls";
import GuardianBattery from "./GuardianBattery";
import { batteryPreset, batteryPresets, recommendedBattery } from "../domain/battery";

afterEach(() => { delete window.CharterCarrier; });
describe("battery settings", () => {
  it("presets update all effective values, advanced changes show Custom and restore resets", () => {
    const host = document.createElement("div"); const root = createRoot(host);
    function Form() { const [value, setValue] = useState(recommendedBattery); return <BatteryControls value={value} onChange={setValue} />; }
    act(() => root.render(<Form />));
    const select = host.querySelector("select")!;
    act(() => { select.value = "Longer battery life"; select.dispatchEvent(new Event("change", { bubbles: true })); });
    expect(host.querySelectorAll("select")[1].value).toBe("600");
    const checkbox = host.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    act(() => checkbox.click()); expect(select.value).toBe("Custom");
    act(() => host.querySelector("button")!.click()); expect(select.value).toBe("Balanced (recommended)");
    expect(host.querySelector("details")!.open).toBe(false);
    act(() => root.unmount());
    for (const [name, value] of Object.entries(batteryPresets)) expect(batteryPreset(value)).toBe(name);
  });
  it("loads native persistence and saves guardian reconnection changes and defaults", () => {
    const save = vi.fn();
    window.CharterCarrier = { isCarrier: () => true, provision: vi.fn(), batterySettings: () => '{"reconnectCapSecs":600}', setBatterySettings: save };
    const host = document.createElement("div"); const root = createRoot(host);
    act(() => root.render(<GuardianBattery />));
    const select = host.querySelector("select")!; expect(select.value).toBe("600");
    act(() => { select.value = "60"; select.dispatchEvent(new Event("change", { bubbles: true })); });
    expect(save).toHaveBeenLastCalledWith('{"reconnectCapSecs":60}');
    act(() => host.querySelector("button")!.click());
    expect(save).toHaveBeenLastCalledWith('{"reconnectCapSecs":300}');
    act(() => root.unmount());
  });
  it("feature detects older APKs without presenting unused controls", () => {
    const host = document.createElement("div"); const root = createRoot(host);
    act(() => root.render(<GuardianBattery />));
    expect(host.querySelector("select")).toBeNull(); expect(host.textContent).toContain("Browser");
    act(() => root.unmount());
  });
});
