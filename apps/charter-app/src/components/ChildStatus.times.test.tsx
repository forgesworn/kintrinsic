// The Now card in a time: "Learning time · 1h 20m left" from the device's own
// STATUS (`activeTime`), with the label looked up in the child's times.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";

const spies = vi.hoisted(() => ({ deviceStatus: {} as Record<string, unknown> }));

vi.mock("../store/store", async (importOriginal) => {
  const real = await importOriginal<typeof import("../store/store")>();
  return { ...real, useCharter: () => ({ ...real.useCharter(), deviceStatus: spies.deviceStatus }) };
});

import { ChildStatus } from "./ChildStatus";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child, NamedTime } from "../domain/types";

const LAPTOP = "a".repeat(64);
const LEARNING: NamedTime = { id: "learning", label: "Learning time", posture: "onlyThese", apps: ["x"] };

const sam = (times: NamedTime[] | undefined): Child => ({
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: LAPTOP }],
  policies: [{ id: "p1", scope: { kind: "device" }, times, schedule: { tz: "UTC", weekly: { mon: [{ start: "09:00", end: "12:00" }] } } }],
});

const status = (over: Record<string, unknown>) => ({
  v: 1,
  subject: "c",
  machine: LAPTOP,
  ts: Math.floor(Date.now() / 1000),
  dayKey: "2026-10-26",
  usedTodaySecs: 0,
  windowLeftSecs: 4800,
  quotaLeftSecs: 4800,
  effectiveSecs: 4830,
  locked: false,
  source: "schedule",
  ...over,
});

describe("Now card, in a time", () => {
  let container: HTMLDivElement;
  let root: Root;
  const mount = (child: Child) => {
    const state: CharterState = { children: [child], requests: [], activity: [], signer: { connected: true, kind: "none", autoSign: true } };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    act(() => {
      root.render(
        <CharterProvider>
          <ChildStatus child={child} variant="page" />
        </CharterProvider>,
      );
    });
  };
  beforeEach(() => {
    localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
  });

  it("says which time and how long is left", () => {
    spies.deviceStatus = { [LAPTOP]: status({ activeTime: "learning" }) };
    mount(sam([LEARNING]));
    expect(container.querySelector(".card-title")?.textContent).toBe("Learning time · 1h 20m left");
    expect(container.textContent).not.toContain("left today");
  });

  it("is the plain card in Free time (no activeTime), exactly as before", () => {
    spies.deviceStatus = { [LAPTOP]: status({}) };
    mount(sam([LEARNING]));
    expect(container.textContent).toContain("1h 20m");
    expect(container.textContent).toContain("left today");
    expect(container.textContent).not.toContain("Learning time");
  });

  it("does not show a bare id for a time that was deleted since", () => {
    spies.deviceStatus = { [LAPTOP]: status({ activeTime: "learning" }) };
    mount(sam([]));
    expect(container.textContent).not.toContain("learning");
    expect(container.textContent).toContain("left today");
  });

  it("with no time limit, names the time alone", () => {
    spies.deviceStatus = { [LAPTOP]: status({ activeTime: "learning", effectiveSecs: 0, windowLeftSecs: 0, source: "unconstrained" }) };
    mount(sam([LEARNING]));
    expect(container.querySelector(".card-title")?.textContent).toBe("Learning time");
  });

  it("a locked device is locked, whatever it last said it was in", () => {
    spies.deviceStatus = { [LAPTOP]: status({ activeTime: "learning", locked: true, effectiveSecs: 0 }) };
    mount(sam([LEARNING]));
    expect(container.textContent).not.toContain("Learning time ·");
  });
});

describe("Now card, in a time that does not use up screen time", () => {
  let container: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
  });
  const mount = (child: Child) => {
    const state: CharterState = { children: [child], requests: [], activity: [], signer: { connected: true, kind: "none", autoSign: true } };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    act(() => {
      root.render(
        <CharterProvider>
          <ChildStatus child={child} variant="page" />
        </CharterProvider>,
      );
    });
  };

  it("reads the span's time when the budget is spent but the time is not locked by it", () => {
    // Quota gone (effective 0 by the budget's arithmetic) yet the device is
    // unlocked in Learning time with 1h20m of its span to run.
    spies.deviceStatus = { [LAPTOP]: status({ activeTime: "learning", effectiveSecs: 0, quotaLeftSecs: 0, windowLeftSecs: 4830 }) };
    mount(sam([LEARNING]));
    expect(container.querySelector(".card-title")?.textContent).toBe("Learning time · 1h 20m left");
  });

  it("a time that DOES use up screen time is limited by what the device says is left", () => {
    spies.deviceStatus = { [LAPTOP]: status({ activeTime: "learning", effectiveSecs: 1830, windowLeftSecs: 7200 }) };
    mount(sam([{ ...LEARNING, countsAgainstBudget: true }]));
    expect(container.querySelector(".card-title")?.textContent).toBe("Learning time · 30m left");
  });
});
