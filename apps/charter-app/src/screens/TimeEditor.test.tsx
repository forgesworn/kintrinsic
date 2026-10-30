// The editor for one time: what it refuses to save, how it surfaces the domain's
// own errors, and exactly what a save hands the store. The store's write is
// spied; everything else is the real provider.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";

const spies = vi.hoisted(() => ({
  savePolicy: vi.fn(),
  deviceStatus: {} as Record<string, unknown>,
}));

vi.mock("../store/store", async (importOriginal) => {
  const real = await importOriginal<typeof import("../store/store")>();
  return {
    ...real,
    useCharter: () => ({ ...real.useCharter(), savePolicy: spies.savePolicy, deviceStatus: spies.deviceStatus }),
  };
});

import TimeEditor from "./TimeEditor";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child, NamedTime, Policy } from "../domain/types";

const LAPTOP = "a".repeat(64);
const PHONE = "b".repeat(64);
const CAPABLE = {
  [LAPTOP]: { ts: Date.now() / 1000, appVersionCode: 800 },
  [PHONE]: { ts: Date.now() / 1000, appVersionCode: 49 },
};

const READING: NamedTime = { id: "reading", label: "Reading time", posture: "onlyThese", apps: ["org.example.books"] };
const LEARNING: NamedTime = {
  id: "learning",
  label: "Learning time",
  posture: "onlyThese",
  sites: [{ id: "khan-academy", label: "Khan Academy", kind: "site", url: "https://www.khanacademy.org/", domains: ["khanacademy.org"] }],
};

function sam(policy: Partial<Policy> = {}, devices?: Child["devices"]): Child {
  return {
    id: "c1",
    name: "Sam",
    color: "#3B6FB2",
    dependantPubkey: null,
    devices: devices ?? [
      { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP },
      { id: "d2", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: PHONE },
    ],
    policies: [
      {
        id: "p1",
        scope: { kind: "device" },
        schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } },
        ...policy,
      },
    ],
  };
}

describe("TimeEditor", () => {
  let container: HTMLDivElement;
  let root: Root;

  const mount = (ui: JSX.Element, child: Child = sam()) => {
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
  const byLabel = (label: string) => container.querySelector<HTMLElement>(`[aria-label="${label}"]`);
  const button = (label: string) => {
    const b = [...container.querySelectorAll("button")].find((el) => el.textContent?.trim() === label);
    if (!b) throw new Error(`no button "${label}" in: ${text()}`);
    return b;
  };
  const click = async (el: HTMLElement) => {
    await act(async () => {
      el.click();
      await new Promise((r) => setTimeout(r, 0));
    });
  };
  const type = async (el: HTMLInputElement, value: string) => {
    await act(async () => {
      const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
      set.call(el, value);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    });
  };

  beforeEach(() => {
    localStorage.clear();
    window.location.hash = "";
    spies.savePolicy.mockReset().mockResolvedValue({ outcome: "sent" });
    spies.deviceStatus = CAPABLE;
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

  describe("refusing what the device would run as nothing", () => {
    it("a new Learning time with no apps or websites says so and cannot be saved", () => {
      mount(<TimeEditor childId="c1" template="learning" />);
      expect(text()).toContain("Choose at least one app or website for this time.");
      expect(button("Save").disabled).toBe(true);
    });

    it("picking a website from the catalogue clears it and saves the definition alone (no hours touched)", async () => {
      mount(<TimeEditor childId="c1" template="learning" />);
      await click(byLabel("Khan Academy in Learning time")!);
      expect(text()).not.toContain("Choose at least one app or website");
      expect(button("Save").disabled).toBe(false);
      await click(button("Save"));

      expect(spies.savePolicy).toHaveBeenCalledTimes(1);
      const [childId, policyId, payload] = spies.savePolicy.mock.calls[0];
      expect([childId, policyId]).toEqual(["c1", "p1"]);
      expect(payload.times).toHaveLength(1);
      expect(payload.times[0]).toMatchObject({
        id: "learning",
        label: "Learning time",
        posture: "onlyThese",
        countsAgainstBudget: false,
      });
      expect(payload.times[0].sites.map((s: { id: string }) => s.id)).toEqual(["khan-academy"]);
      // No hours changed: no schedule, no per-device schedules are signed.
      expect(payload.schedule).toBeUndefined();
      expect(payload.deviceOverrides).toBeUndefined();
      expect(window.location.hash).toBe("#/ward/c1/settings");
    });

    it("seeds the Learning template from the sites already trusted for study", () => {
      mount(
        <TimeEditor childId="c1" template="learning" />,
        sam({
          learning: {
            enabled: true,
            apps: [{ id: "wikipedia", label: "Wikipedia", kind: "site", url: "https://www.wikipedia.org/", domains: ["wikipedia.org", "wikimedia.org"] }],
          },
        }),
      );
      expect(byLabel("Wikipedia in Learning time")!.getAttribute("aria-checked")).toBe("true");
      expect(button("Save").disabled).toBe(false);
    });

    it("a blank name is refused", async () => {
      mount(<TimeEditor childId="c1" template="custom" name="Reading time" />);
      await click(byLabel("Khan Academy in Reading time")!);
      expect(button("Save").disabled).toBe(false);
      await type(container.querySelector<HTMLInputElement>("#time-name")!, "   ");
      expect(text()).toContain("Give this time a name.");
      expect(button("Save").disabled).toBe(true);
    });

    it("a website typed with a bad address is refused with a reason, and not added", async () => {
      mount(<TimeEditor childId="c1" template="custom" name="Reading time" />);
      await type(container.querySelector<HTMLInputElement>('input[aria-label="Site name"]')!, "Maths");
      await type(container.querySelector<HTMLInputElement>('input[aria-label="Site address"]')!, "localhost");
      await click(button("Add site"));
      expect(text()).not.toContain("Maths");
      expect(button("Save").disabled).toBe(true);
    });

    it("a website typed with a good address is added as a valid https site", async () => {
      mount(<TimeEditor childId="c1" template="custom" name="Reading time" />);
      await type(container.querySelector<HTMLInputElement>('input[aria-label="Site name"]')!, "Maths Genie");
      await type(container.querySelector<HTMLInputElement>('input[aria-label="Site address"]')!, "http://mathsgenie.co.uk");
      await click(button("Add site"));
      await click(button("Save"));
      const site = spies.savePolicy.mock.calls[0][2].times[0].sites[0];
      expect(site).toMatchObject({ id: "maths-genie", kind: "site", domains: ["mathsgenie.co.uk"] });
      expect(site.url.startsWith("https://")).toBe(true);
    });
  });

  describe("custom times", () => {
    it("makes its id from the name, and saves it when something is allowed", async () => {
      mount(<TimeEditor childId="c1" template="custom" name="Reading time" />);
      await click(byLabel("Wikipedia in Reading time")!);
      await click(button("Save"));
      expect(spies.savePolicy.mock.calls[0][2].times[0]).toMatchObject({ id: "reading-time", label: "Reading time" });
    });

    it("never takes an id another time already has", async () => {
      mount(<TimeEditor childId="c1" template="custom" name="Reading time" />, sam({ times: [{ ...READING, id: "reading-time" }] }));
      await click(byLabel("Wikipedia in Reading time")!);
      await click(button("Save"));
      const ids = spies.savePolicy.mock.calls[0][2].times.map((t: NamedTime) => t.id);
      expect(ids).toEqual(["reading-time", "reading-time-2"]);
    });
  });

  describe("hours", () => {
    it("writes tagged windows into the chosen device's own schedule, and signs the schedule with them", async () => {
      mount(<TimeEditor childId="c1" template="learning" />, sam({ learning: { enabled: true, apps: LEARNING.sites! } }));
      // The laptop's Monday toggle (the first "Run Learning time on Monday").
      await click(byLabel("Run Learning time every day")!);
      await click(button("Save"));
      const payload = spies.savePolicy.mock.calls[0][2];
      expect(payload.times.map((t: NamedTime) => t.id)).toEqual(["learning"]);
      // Every device was seeded when the schedule split; the laptop alone has hours.
      expect(Object.keys(payload.deviceOverrides).sort()).toEqual(["d1", "d2"]);
      expect(payload.deviceOverrides.d1.schedule.weekly.mon).toContainEqual({ start: "16:00", end: "18:00", time: "learning" });
      expect(payload.deviceOverrides.d2.schedule.weekly.mon).toEqual([{ start: "16:00", end: "19:00" }]);
      // The base schedule rides along: without it `pickOverrides` would sign none of these.
      expect(payload.schedule).toBeDefined();
    });

    it("surfaces the domain's overlap sentence and refuses to save when two times collide on a device", async () => {
      const child = sam({
        times: [READING],
        deviceOverrides: {
          d1: { schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "18:00", time: "reading" }] } } },
        },
      });
      mount(<TimeEditor childId="c1" template="custom" name="Maths" />, child);
      await click(byLabel("Wikipedia in Maths")!);
      // Two devices -> two editors. The default window is 16:00-18:00 on Monday...
      await click(container.querySelector<HTMLElement>('[aria-label="Run Maths every day"]')!);
      expect(container.querySelector('[role="alert"]')?.textContent).toMatch(/Two times overlap on mon/);
      expect(button("Save").disabled).toBe(true);
    });

    it("lets a time overlap Free time and says it carves", async () => {
      mount(<TimeEditor childId="c1" template="learning" />, sam({ learning: { enabled: true, apps: LEARNING.sites! } }));
      expect(text()).toContain("this time carves out of it");
      await click(byLabel("Run Learning time every day")!);
      // 16:00-18:00 sits inside Free time's 16:00-19:00 and is not an error.
      expect(container.querySelector('[role="alert"]')).toBeNull();
      expect(button("Save").disabled).toBe(false);
    });

    it("lists only devices that can run times, and says the others need updating", () => {
      spies.deviceStatus = { ...CAPABLE, [PHONE]: { ts: Date.now() / 1000, appVersionCode: 40 } };
      mount(<TimeEditor childId="c1" template="learning" />);
      expect(text()).toContain("Sam's phone needs updating to use times");
      expect(text()).toContain("Sam's laptop");
      expect(container.querySelectorAll('[aria-label="Run Learning time every day"]')).toHaveLength(1);
    });

    it("asks for a schedule first when Free time has none", async () => {
      const child = sam({ schedule: undefined });
      mount(<TimeEditor childId="c1" template="learning" />, child);
      await click(byLabel("Khan Academy in Learning time")!);
      await click(container.querySelector<HTMLElement>('[aria-label="Run Learning time every day"]')!);
      expect(container.querySelector('[role="alert"]')?.textContent).toMatch(/schedule/);
      expect(button("Save").disabled).toBe(true);
    });
  });

  describe("an existing time", () => {
    const withLearning = () =>
      sam({
        times: [LEARNING],
        deviceOverrides: {
          d1: {
            schedule: {
              tz: "UTC",
              weekly: { mon: [{ start: "09:00", end: "11:00", time: "learning" }] },
            },
          },
          d2: { schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } } },
        },
      });

    it("shows its hours on the device and starts clean", () => {
      mount(<TimeEditor childId="c1" timeId="learning" />, withLearning());
      expect(button("Save").disabled).toBe(true);
      const starts = [...container.querySelectorAll<HTMLInputElement>('input[aria-label="Monday start time"]')].map((i) => i.value);
      expect(starts).toContain("09:00");
    });

    it("renaming changes the name only, and signs the times without touching any hours", async () => {
      mount(<TimeEditor childId="c1" timeId="learning" />, withLearning());
      await type(container.querySelector<HTMLInputElement>("#time-name")!, "Study time");
      await click(button("Save"));
      const payload = spies.savePolicy.mock.calls[0][2];
      expect(payload.times[0]).toMatchObject({ id: "learning", label: "Study time" });
      expect(payload.schedule).toBeUndefined();
      expect(payload.deviceOverrides).toBeUndefined();
    });

    it("says plainly when the time no longer exists", () => {
      mount(<TimeEditor childId="c1" timeId="nope" />);
      expect(text()).toContain("That time isn’t set up any more.");
    });

    it("deleting warns about a device left with no hours, then removes the time and its hours in one save", async () => {
      // The laptop's only hours are Learning's.
      const child = withLearning();
      mount(<TimeEditor childId="c1" timeId="learning" />, child);
      await click(button("Delete this time"));
      expect(text()).toContain("Sam's laptop would have no allowed time at all");
      await click(button("Delete Learning time"));

      expect(spies.savePolicy).toHaveBeenCalledTimes(1);
      const payload = spies.savePolicy.mock.calls[0][2];
      expect(payload.times).toEqual([]);
      expect(payload.deviceOverrides.d1.schedule.weekly.mon ?? []).toEqual([]);
      expect(payload.deviceOverrides.d2.schedule.weekly.mon).toEqual([{ start: "16:00", end: "19:00" }]);
      expect(payload.schedule).toBeDefined();
    });

    it("deleting a time with no hours anywhere signs only the new set", async () => {
      mount(<TimeEditor childId="c1" timeId="learning" />, sam({ times: [LEARNING, READING] }));
      await click(button("Delete this time"));
      expect(text()).not.toContain("would have no allowed time");
      await click(button("Delete Learning time"));
      const payload = spies.savePolicy.mock.calls[0][2];
      expect(payload.times.map((t: NamedTime) => t.id)).toEqual(["reading"]);
      expect(payload.schedule).toBeUndefined();
    });

    it("'Keep it' backs out of a delete", async () => {
      mount(<TimeEditor childId="c1" timeId="learning" />, withLearning());
      await click(button("Delete this time"));
      await click(button("Keep it"));
      expect(spies.savePolicy).not.toHaveBeenCalled();
      expect(text()).toContain("Delete this time");
    });
  });

  it("a save the signer was declined keeps the draft on the page", async () => {
    spies.savePolicy.mockResolvedValue({ outcome: "cancelled" });
    window.location.hash = "/ward/c1/time/new/learning";
    mount(<TimeEditor childId="c1" template="learning" />);
    await click(byLabel("Khan Academy in Learning time")!);
    await click(button("Save"));
    expect(window.location.hash).toBe("#/ward/c1/time/new/learning");
    expect(button("Save").disabled).toBe(false);
  });
});
