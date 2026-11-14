import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSection } from "../domain/deviceApps";

vi.mock("../store/store", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../store/store")>()),
  useCharter: () => ({}),
}));

import { AlwaysAvailableEditor } from "./Limits";

const SENTENCE = "Anyone holding this device can use its open-at-any-hour apps.";
const section = (deviceId: string, label: string): AppSection => ({
  deviceId,
  label,
  platform: "android",
  apps: [{ pkg: `com.${deviceId}.player`, label: "Audiobooks" }],
});

describe("open-at-any-hour picker, for a device with no screen lock", () => {
  let container: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    container = document.createElement("div");
    document.body.append(container);
  });
  afterEach(() => { act(() => root.unmount()); container.remove(); });

  const mount = (noLock?: ReadonlySet<string>) =>
    act(() => {
      root = createRoot(container);
      root.render(
        <AlwaysAvailableEditor
          alwaysAvailable={{ apps: [] }}
          sections={[section("d1", "Robin's phone"), section("d2", "Robin's tablet")]}
          onChange={() => {}}
          wardName="Robin"
          noScreenLockDeviceIds={noLock}
        />,
      );
    });

  it("says so beside that device's apps, and only that device's", () => {
    mount(new Set(["d1"]));
    const text = container.textContent ?? "";
    expect(text.split(SENTENCE)).toHaveLength(2); // exactly once
    expect(text.indexOf("Robin's phone")).toBeLessThan(text.indexOf(SENTENCE));
    expect(text.indexOf(SENTENCE)).toBeLessThan(text.indexOf("Robin's tablet"));
  });

  it("says nothing when every device has a screen lock", () => {
    mount(new Set());
    expect(container.textContent).not.toContain(SENTENCE);
    act(() => root.unmount());
    mount(undefined);
    expect(container.textContent).not.toContain(SENTENCE);
  });
});
