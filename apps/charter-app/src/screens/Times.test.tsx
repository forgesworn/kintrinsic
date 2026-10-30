// The Times section on a child's settings: Free time first, a card per time, the
// way to add one, and the gate for devices that cannot run times yet.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";

const spies = vi.hoisted(() => ({ deviceStatus: {} as Record<string, unknown> }));

vi.mock("../store/store", async (importOriginal) => {
  const real = await importOriginal<typeof import("../store/store")>();
  return { ...real, useCharter: () => ({ ...real.useCharter(), deviceStatus: spies.deviceStatus }) };
});

import ChildSettings from "./ChildSettings";
import { TimesSection } from "./Times";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child, NamedTime } from "../domain/types";

const LAPTOP = "a".repeat(64);
const PHONE = "b".repeat(64);
const LEARNING: NamedTime = {
  id: "learning",
  label: "Learning time",
  posture: "onlyThese",
  sites: [{ id: "khan-academy", label: "Khan Academy", kind: "site", url: "https://www.khanacademy.org/", domains: ["khanacademy.org"] }],
};

const sam = (times?: NamedTime[], extra: Partial<Child["policies"][number]> = {}): Child => ({
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [
    { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP },
    { id: "d2", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE },
  ],
  policies: [
    {
      id: "p1",
      scope: { kind: "device" },
      times,
      schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } },
      deviceOverrides: {
        d1: {
          schedule: {
            tz: "UTC",
            weekly: {
              mon: [{ start: "09:00", end: "11:00", time: "learning" }, { start: "16:00", end: "19:00" }],
              tue: [{ start: "09:00", end: "11:00", time: "learning" }],
            },
          },
        },
        d2: { schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } } },
      },
      ...extra,
    },
  ],
});

describe("Times section", () => {
  let container: HTMLDivElement;
  let root: Root;
  const mount = (ui: JSX.Element, child: Child = sam([LEARNING])) => {
    const state: CharterState = {
      children: [child],
      requests: [],
      activity: [],
      signer: { connected: true, kind: "none", autoSign: true },
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    act(() => {
      root.render(<CharterProvider>{ui}</CharterProvider>);
    });
  };
  const text = () => container.textContent ?? "";
  const button = (label: string) => {
    const b = [...container.querySelectorAll("button")].find((el) => el.textContent?.trim() === label);
    if (!b) throw new Error(`no button "${label}" in: ${text()}`);
    return b;
  };
  /** The card (a row button) whose title is `title`. */
  const card = (title: string) => {
    const t = [...container.querySelectorAll(".row-title")].find((e) => e.textContent === title);
    if (!t) throw new Error(`no card "${title}" in: ${text()}`);
    return t.closest("button") as HTMLButtonElement;
  };
  const click = async (el: HTMLElement) => {
    await act(async () => {
      el.click();
    });
  };
  const type = async (el: HTMLInputElement, value: string) => {
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(el, value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });
  };

  beforeEach(() => {
    localStorage.clear();
    window.location.hash = "";
    spies.deviceStatus = {};
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
    window.location.hash = "";
  });

  it("shows Free time first, then a card per time with each device's hours", () => {
    mount(<TimesSection child={sam([LEARNING])} />);
    const titles = [...container.querySelectorAll(".row-title")].map((e) => e.textContent);
    expect(titles).toEqual(["Free time", "Learning time"]);
    expect(text()).toContain("Only Khan Academy");
    expect(text()).toContain("Sam's laptop: Mon, Tue 09:00–11:00");
    expect(text()).toContain("Sam's phone: No hours yet");
  });

  it("a card opens that time's editor; Free time opens its schedule", async () => {
    mount(<TimesSection child={sam([LEARNING])} />);
    await click(card("Learning time"));
    expect(window.location.hash).toBe("#/ward/c1/time/learning");
    await click(card("Free time"));
    expect(window.location.hash).toBe("#/ward/c1/rule/schedule");
  });

  it("Add a time → Learning time opens the template; it is not offered twice", async () => {
    mount(<TimesSection child={sam([])} />);
    await click(button("Add a time"));
    await click(button("Learning time"));
    expect(window.location.hash).toBe("#/ward/c1/time/new/learning");
    // With a Learning time already there, the template is not offered again.
    act(() => root.unmount());
    root = createRoot(container);
    mount(<TimesSection child={sam([LEARNING])} />);
    await click(button("Add a time"));
    expect(container.querySelector(".card")?.textContent).not.toContain("Learning time");
    expect(button("Custom")).toBeTruthy();
  });

  it("Add a time → Custom takes a name and opens the editor with it", async () => {
    mount(<TimesSection child={sam([])} />);
    await click(button("Add a time"));
    expect(button("Custom").disabled).toBe(true);
    await type(container.querySelector<HTMLInputElement>('input[placeholder^="Name it"]')!, "Reading time");
    await click(button("Custom"));
    expect(window.location.hash).toBe("#/ward/c1/time/new/custom/Reading%20time");
  });

  it("stops at the most times a child can have", () => {
    const sixteen = Array.from({ length: 16 }, (_, i) => ({ ...LEARNING, id: `t${i}`, label: `Time ${i}` }));
    mount(<TimesSection child={sam(sixteen)} />);
    expect(button("Add a time").disabled).toBe(true);
    expect(text()).toContain("most times a child can have (16)");
  });

  describe("a device that cannot run times yet", () => {
    it("is named, told to update, and offered no hours", () => {
      spies.deviceStatus = { [PHONE]: { ts: Date.now() / 1000, appVersionCode: 40 } };
      mount(<TimesSection child={sam([LEARNING])} />);
      expect(text()).toContain("Sam's phone needs updating to use times");
      expect(text()).toContain("nothing you set for another time reaches it");
      // Its hours line is not on the card at all.
      expect(text()).not.toContain("Sam's phone: ");
      expect(text()).toContain("Sam's laptop: ");
    });

    it("says nothing when every device is fine, or has not reported yet", () => {
      mount(<TimesSection child={sam([LEARNING])} />);
      expect(text()).not.toContain("needs updating");
    });
  });

  it("warns when hours are set for a time that no longer exists (they stay locked)", () => {
    mount(<TimesSection child={sam([])} />);
    expect(text()).toContain("isn’t set up any more, so those hours stay locked");
  });

  it("sits first on the child's settings, and the Rules under it are Free time's", () => {
    mount(<ChildSettings child={sam([LEARNING])} />);
    const labels = [...container.querySelectorAll(".section-label")].map((e) => e.textContent);
    expect(labels.slice(0, 2)).toEqual(["Times", "Free time’s rules"]);
    expect(text()).toContain("What’s banned in Free time");
    // The old screen is now App allowances, under the apps.
    expect(text()).toContain("App allowances");
    expect(text()).not.toMatch(/named times?/i);
  });
});
