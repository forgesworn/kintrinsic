// Free time's schedule editor and the windows that run other times. The editor
// edits Free time's hours; a tagged window passing through it must come out the
// other side tagged, in place, or one save of the daily schedule would turn
// every Learning window into Free time (a loosening nobody asked for).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";

const spies = vi.hoisted(() => ({ savePolicy: vi.fn() }));

vi.mock("../store/store", async (importOriginal) => {
  const real = await importOriginal<typeof import("../store/store")>();
  return { ...real, useCharter: () => ({ ...real.useCharter(), savePolicy: spies.savePolicy }) };
});

import RuleEditor from "./RuleEditor";
import { buildSchedule, scheduleSummary, ScheduleEditor } from "./Limits";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child, NamedTime, Schedule } from "../domain/types";

const LEARNING: NamedTime = { id: "learning", label: "Learning time", posture: "onlyThese", apps: ["x"] };

const withTag: Schedule = {
  tz: "UTC",
  weekly: {
    mon: [{ start: "09:00", end: "11:00", time: "learning" }, { start: "16:00", end: "19:00" }],
    tue: [{ start: "09:00", end: "11:00", time: "learning" }],
    wed: [{ start: "16:00", end: "19:00" }],
    thu: [{ start: "16:00", end: "19:00" }],
    fri: [{ start: "16:00", end: "19:00" }],
    sat: [{ start: "16:00", end: "19:00" }],
    sun: [{ start: "16:00", end: "19:00" }],
  },
};

describe("a schedule with windows for another time", () => {
  it("the editor's working copy keeps the tag (and adds no key to an untagged window)", () => {
    const built = buildSchedule(withTag);
    expect(built.weekly.mon).toEqual([
      { start: "09:00", end: "11:00", time: "learning" },
      { start: "16:00", end: "19:00" },
    ]);
    expect("time" in built.weekly.mon![1]).toBe(false);
    // So an untouched screen is not dirty.
    expect(JSON.stringify(buildSchedule(built))).toBe(JSON.stringify(built));
  });

  it("the summary counts Free time's days only", () => {
    expect(scheduleSummary(buildSchedule(withTag))).toBe("Allowed 6 days a week");
    const onlyTagged: Schedule = { tz: "UTC", weekly: { mon: [{ start: "09:00", end: "11:00", time: "learning" }] } };
    expect(scheduleSummary(buildSchedule(onlyTagged))).toBe("Off — only other times run");
  });
});

describe("ScheduleEditor", () => {
  let container: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });
  const click = async (el: HTMLElement) => {
    await act(async () => {
      el.click();
    });
  };

  it("draws the other time's hours apart, under its name", () => {
    act(() => root.render(<ScheduleEditor schedule={withTag} onChange={() => {}} times={[LEARNING]} />));
    const list = container.querySelector(".tagged-windows")!;
    expect([...list.querySelectorAll("li")].map((li) => li.textContent)).toEqual([
      "Learning time · Mon 09:00–11:00",
      "Learning time · Tue 09:00–11:00",
    ]);
    expect(container.textContent).toContain("carves out of it");
  });

  it("names a tag the child has no definition for rather than showing a bare id", () => {
    act(() => root.render(<ScheduleEditor schedule={withTag} onChange={() => {}} times={[]} />));
    expect(container.querySelector(".tagged-windows")!.textContent).toContain("A time that isn’t set up");
  });

  it("turning Free time off for every day leaves the tagged windows exactly where they were", async () => {
    const onChange = vi.fn();
    // Wed-Sun are Free-only and Mon differs from Wed, so the editor opens per-day.
    act(() => root.render(<ScheduleEditor schedule={withTag} onChange={onChange} times={[LEARNING]} />));
    await click(container.querySelector<HTMLElement>('[aria-label="Allow screen time on Monday"]')!);
    const next = onChange.mock.calls[0][0] as Schedule;
    // Monday's Free hours went; its Learning window stayed.
    expect(next.weekly.mon).toEqual([{ start: "09:00", end: "11:00", time: "learning" }]);
    expect(next.weekly.tue).toEqual([{ start: "09:00", end: "11:00", time: "learning" }]);
    expect(next.weekly.wed).toEqual([{ start: "16:00", end: "19:00" }]);
  });

  it("'same every day' never overwrites a tagged window", async () => {
    const onChange = vi.fn();
    const freeOnlyEvery: Schedule = {
      ...withTag,
      weekly: {
        ...withTag.weekly,
        mon: [{ start: "09:00", end: "11:00", time: "learning" }, { start: "16:00", end: "19:00" }],
        tue: [{ start: "09:00", end: "11:00", time: "learning" }, { start: "16:00", end: "19:00" }],
      },
    };
    act(() => root.render(<ScheduleEditor schedule={freeOnlyEvery} onChange={onChange} times={[LEARNING]} />));
    // Free time reads the same every day, so the simple view opens.
    await click(container.querySelector<HTMLElement>('[aria-label="Allow screen time every day"]')!);
    const next = onChange.mock.calls[0][0] as Schedule;
    for (const d of ["mon", "tue"] as const) {
      expect(next.weekly[d]).toEqual([{ start: "09:00", end: "11:00", time: "learning" }]);
    }
    for (const d of ["wed", "thu", "fri", "sat", "sun"] as const) expect(next.weekly[d]).toEqual([]);
  });

  it("a plain schedule behaves as it always did", () => {
    act(() =>
      root.render(
        <ScheduleEditor schedule={buildSchedule({ tz: "UTC", weekly: { mon: [{ start: "16:00", end: "18:00" }] } })} onChange={() => {}} />,
      ),
    );
    expect(container.querySelector(".tagged-windows")).toBeNull();
    expect(container.textContent).toContain("Set each day separately");
  });

  it("with a kind it talks about running, not allowing", () => {
    act(() =>
      root.render(
        <ScheduleEditor
          kind="Learning time"
          schedule={buildSchedule({ tz: "UTC", weekly: { mon: [{ start: "09:00", end: "11:00" }] } })}
          onChange={() => {}}
        />,
      ),
    );
    expect(container.querySelector('[aria-label="Run Learning time on Monday"]')).not.toBeNull();
    expect(container.textContent).toContain("Runs");
    expect(container.textContent).not.toContain("Allowed");
  });
});

describe("the Daily schedule page on a child whose devices run other times", () => {
  let container: HTMLDivElement;
  let root: Root;
  const child = (): Child => ({
    id: "c1",
    name: "Sam",
    color: "#3B6FB2",
    dependantPubkey: null,
    devices: [
      { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) },
      { id: "d2", label: "Sam's phone", platform: "android", pairing: "paired", devicePubkey: "b".repeat(64) },
    ],
    policies: [
      {
        id: "p1",
        scope: { kind: "device" },
        times: [LEARNING],
        schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } },
        deviceOverrides: {
          d1: { schedule: withTag },
          d2: { schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "19:00" }] } } },
        },
      },
    ],
  });
  const mount = () => {
    const state: CharterState = { children: [child()], requests: [], activity: [], signer: { connected: true, kind: "none", autoSign: true } };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    act(() => {
      root.render(
        <CharterProvider>
          <RuleEditor childId="c1" section="schedule" />
        </CharterProvider>,
      );
    });
  };

  beforeEach(() => {
    localStorage.clear();
    spies.savePolicy.mockReset().mockResolvedValue({ outcome: "sent" });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
  });

  it("does not offer 'Off unless I open it', which would wipe the other time's hours", () => {
    mount();
    expect(container.querySelector('[aria-label="Off unless I open it"]')).toBeNull();
  });

  it("keeps each device on its own schedule, and says why, rather than flattening them into one", () => {
    mount();
    const toggle = container.querySelector<HTMLButtonElement>('[aria-label="Set separately for each device"]')!;
    expect(toggle.disabled).toBe(true);
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toContain("Another time has hours on a device");
  });

  it("saving a change to Free time's hours signs the per-device schedules with their tags intact", async () => {
    mount();
    const tuesdayFree = container.querySelector<HTMLElement>('[aria-label="Allow screen time on Tuesday"]')!;
    await act(async () => {
      tuesdayFree.click();
    });
    const save = [...container.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Save")!;
    await act(async () => {
      save.click();
    });
    expect(spies.savePolicy).toHaveBeenCalledTimes(1);
    const payload = spies.savePolicy.mock.calls[0][2];
    expect(payload.deviceOverrides.d1.schedule.weekly.mon).toContainEqual({ start: "09:00", end: "11:00", time: "learning" });
    expect(payload.deviceOverrides.d1.schedule.weekly.tue).toContainEqual({ start: "09:00", end: "11:00", time: "learning" });
  });
});
