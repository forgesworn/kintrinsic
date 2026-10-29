import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { BottomBar } from "./BottomBar";
import { bottomBarFor } from "./bottomBarFor";
import { CharterProvider, STORAGE_KEY } from "../store/store";
import type { Child, ChildRequest } from "../domain/types";

const kid = (id: string, name: string, color: string): Child => ({
  id,
  name,
  color,
  dependantPubkey: null,
  devices: [{ id: `${id}-d`, label: "Laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) }],
  policies: [{ id: `${id}-p`, scope: { kind: "device" } }],
});
const FAMILY = [kid("c2", "Ella", "#C4860F"), kid("c1", "Sam", "#3B6FB2"), kid("c3", "Family laptop", "#2E8B57")];

const ask = (id: string, childId: string, status: ChildRequest["status"] = "pending"): ChildRequest => ({
  id,
  childId,
  requester: "device",
  deviceId: `${childId}-d`,
  kind: "time.extend",
  createdAt: Date.now(),
  title: "asks",
  minutesRequested: 15,
  status,
});

describe("BottomBar", () => {
  let container: HTMLDivElement;
  let root: Root;

  const mount = (route: string, requests: ChildRequest[] = [], children: Child[] = FAMILY) => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children,
        requests,
        activity: [],
        signer: { connected: false, kind: "local", autoSign: false },
      }),
    );
    act(() => {
      root.render(
        <CharterProvider>
          <BottomBar route={route} />
        </CharterProvider>,
      );
    });
  };
  const buttons = () => [...container.querySelectorAll<HTMLButtonElement>("nav button")];
  const current = () =>
    buttons()
      .filter((b) => b.getAttribute("aria-current") === "page")
      .map((b) => b.querySelector(".tab-label")?.textContent);

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
    window.location.hash = "";
  });

  it("is the Main nav: the children in the family's order, then Settings; no Home", () => {
    mount("ward/c1");
    expect(container.querySelector('nav[aria-label="Main"]')).not.toBeNull();
    expect(buttons().map((b) => b.querySelector(".tab-label")?.textContent)).toEqual([
      "Ella",
      "Sam",
      "Family laptop",
      "Settings",
    ]);
  });

  it("with no children it is Home and Settings, and Home is current on the getting-started page", () => {
    mount("home", [], []);
    expect(buttons().map((b) => b.querySelector(".tab-label")?.textContent)).toEqual(["Home", "Settings"]);
    expect(current()).toEqual(["Home"]);
  });

  it("shows a count badge for a child with asks waiting, and none otherwise", () => {
    mount("settings", [ask("r1", "c1"), ask("r2", "c1"), ask("r3", "c2", "approved")]);
    const badge = (name: string) =>
      buttons().find((b) => b.textContent?.includes(name))?.querySelector(".badge")?.textContent;
    expect(badge("Sam")).toBe("2");
    expect(badge("Ella")).toBeUndefined();
    expect(badge("Family laptop")).toBeUndefined();
  });

  it("names each child's state in the button's label, and the waiting asks once", () => {
    mount("settings", [ask("r1", "c1"), ask("r2", "c1"), ask("r3", "c2", "approved")]);
    const button = (name: string) => buttons().find((b) => b.querySelector(".tab-label")?.textContent === name)!;
    expect(button("Ella").getAttribute("aria-label")).toBe("Ella, offline, no time limit today");
    expect(button("Sam").getAttribute("aria-label")).toBe("Sam, offline, no time limit today, 2 waiting");
    // The visual badge is hidden from assistive tech: the label already says it.
    expect(button("Sam").querySelector(".tab-badge")?.closest('[aria-hidden="true"]')).not.toBeNull();
    expect(container.querySelector('[role="img"]')).toBeNull();
  });

  it("shows the glance line and an online dot only for a device seen lately", () => {
    const seen = (id: string, name: string, color: string, ago: number): Child => ({
      ...kid(id, name, color),
      devices: [
        {
          id: `${id}-d`,
          label: "Laptop",
          platform: "linux",
          pairing: "paired",
          devicePubkey: "a".repeat(64),
          lastSeenAt: Date.now() - ago,
        },
      ],
      policies: [{ id: `${id}-p`, scope: { kind: "device" }, budget: { tz: "UTC", dailyMinutes: 120 } }],
    });
    mount("ward/c1", [], [seen("c1", "Sam", "#3B6FB2", 30_000), seen("c2", "Ella", "#C4860F", 3_600_000)]);
    const button = (name: string) => buttons().find((b) => b.querySelector(".tab-label")?.textContent === name)!;
    expect(button("Sam").querySelector(".tab-dot")?.getAttribute("data-online")).toBe("true");
    expect(button("Ella").querySelector(".tab-dot")?.hasAttribute("data-online")).toBe(false);
    expect(button("Sam").getAttribute("aria-label")).toMatch(/^Sam, online, /);
    expect(button("Ella").getAttribute("aria-label")).toMatch(/^Ella, offline, /);
    expect(button("Sam").querySelector(".tab-glance")?.textContent).toMatch(/left$/);
    expect(button("Sam").querySelector(".tab-meter")).not.toBeNull();
  });

  it("redraws the glance every 30 seconds, and stops when it unmounts", () => {
    vi.useFakeTimers();
    try {
      const start = Date.now();
      const sam: Child = {
        ...kid("c1", "Sam", "#3B6FB2"),
        devices: [
          {
            id: "c1-d",
            label: "Laptop",
            platform: "linux",
            pairing: "paired",
            devicePubkey: "a".repeat(64),
            lastSeenAt: start,
          },
        ],
      };
      const set = vi.spyOn(globalThis, "setInterval");
      const clear = vi.spyOn(globalThis, "clearInterval");
      mount("ward/c1", [], [sam]);
      const tick = set.mock.calls.findIndex(([, ms]) => ms === 30_000);
      expect(tick).toBeGreaterThanOrEqual(0);
      const tickId = set.mock.results[tick].value;
      const label = () => buttons()[0].getAttribute("aria-label");
      expect(label()).toBe("Sam, online, no time limit today");
      // Five minutes on with no word from the device, it is offline.
      act(() => vi.advanceTimersByTime(5 * 60_000 + 30_000));
      expect(label()).toBe("Sam, offline, no time limit today");
      act(() => root.unmount());
      expect(clear).toHaveBeenCalledWith(tickId);
      root = createRoot(container);
    } finally {
      vi.restoreAllMocks();
      vi.useRealTimers();
    }
  });

  it("marks the child on any of their pages, or Settings, as current", () => {
    const cases: [string, string[]][] = [
      ["home", ["Ella"]],
      ["", ["Ella"]],
      ["approvals", ["Ella"]],
      ["ward/ghost", ["Ella"]],
      ["ward/c1", ["Sam"]],
      ["ward/c1/settings", ["Sam"]],
      ["settings", ["Settings"]],
      ["settings/key-backup", ["Settings"]],
      ["activity", []],
    ];
    for (const [route, expected] of cases) {
      act(() => root.unmount());
      root = createRoot(container);
      mount(route);
      expect(current(), route).toEqual(expected);
    }
  });

  it("goes to a child's page and Settings", () => {
    mount("settings");
    const click = (name: string) =>
      act(() => buttons().find((b) => b.textContent?.includes(name))!.click());
    click("Ella");
    expect(window.location.hash).toBe("#/ward/c2");
    click("Settings");
    expect(window.location.hash).toBe("#/settings");
  });

  it("goes to Home from the getting-started bar when there are no children", () => {
    mount("settings", [], []);
    act(() => buttons().find((b) => b.textContent?.includes("Home"))!.click());
    expect(window.location.hash).toBe("#/home");
  });

  it("renders nothing on the full-screen editors, set-up or an app rule", () => {
    for (const route of [
      "ward/c1/rule/budget",
      "ward/c1/rule/app/c1-p",
      "ward/c1/device/c1-d",
      "ward/c1/add-device",
      "setup",
    ]) {
      act(() => root.unmount());
      root = createRoot(container);
      mount(route);
      expect(container.querySelector("nav"), route).toBeNull();
    }
  });
});

describe("bottomBarFor", () => {
  it("is shown on the child pages, Settings and Activity (and Home with no children)", () => {
    expect(bottomBarFor("home", []).show).toBe(true);
    for (const r of ["home", "", "ward/c1", "ward/c1/settings", "settings", "settings/key-backup", "activity/c1"]) {
      expect(bottomBarFor(r, FAMILY).show, r).toBe(true);
    }
  });
  it("names the current child, Settings, or Home (only with no children)", () => {
    expect(bottomBarFor("ward/c2/settings", FAMILY).active).toEqual({ childId: "c2" });
    expect(bottomBarFor("home", FAMILY).active).toEqual({ childId: "c2" });
    expect(bottomBarFor("home", []).active).toBe("home");
    expect(bottomBarFor("settings", FAMILY).active).toBe("settings");
    expect(bottomBarFor("activity", FAMILY).active).toBeNull();
  });
  it("treats a child who no longer exists as the first child, or Home with none", () => {
    expect(bottomBarFor("ward/ghost/rule/budget", FAMILY)).toEqual({ show: true, active: { childId: "c2" } });
    expect(bottomBarFor("ward/ghost/rule/budget", [])).toEqual({ show: true, active: "home" });
  });
  it("marks the first child with an ask waiting for the bare #/approvals link", () => {
    expect(bottomBarFor("approvals", FAMILY, new Set(["c1"])).active).toEqual({ childId: "c1" });
  });
});
