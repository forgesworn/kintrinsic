// The shell and the route table, mounted for real: where each old and new
// address lands, what the header says, and what the back arrow does.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import App from "./App";
import { CharterProvider, STORAGE_KEY, type CharterState } from "./store/store";
import type { Child, ChildRequest } from "./domain/types";

const SAM: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [
    { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) },
  ],
  policies: [{ id: "p1", scope: { kind: "device" } }],
};

const ASK: ChildRequest = {
  id: "r1",
  childId: "c1",
  requester: "device",
  deviceId: "d1",
  kind: "time.extend",
  createdAt: Date.now() - 60_000,
  title: "Sam's laptop asks for 15 more minutes",
  minutesRequested: 15,
  status: "pending",
};

const ELLA: Child = {
  ...SAM,
  id: "c2",
  name: "Ella",
  devices: [
    { id: "d2", label: "Ella's laptop", platform: "linux", pairing: "paired", devicePubkey: "b".repeat(64) },
  ],
};

const ELLA_ASK: ChildRequest = {
  ...ASK,
  id: "r2",
  childId: "c2",
  deviceId: "d2",
  title: "Ella's laptop asks for 15 more minutes",
};

/** Sam and Ella, with an ask waiting from Ella only. */
function twoChildren(): CharterState {
  return { ...seeded([ELLA_ASK]), children: [SAM, ELLA] };
}

function seeded(requests: ChildRequest[] = [ASK]): CharterState {
  return {
    children: [SAM],
    requests,
    activity: [],
    signer: { connected: true, kind: "none", autoSign: false },
  };
}

describe("App shell and routes", () => {
  let container: HTMLDivElement;
  let root: Root;

  const mount = (hash: string, state: CharterState = seeded()) => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    window.location.hash = hash;
    act(() => {
      root.render(
        <CharterProvider>
          <App />
        </CharterProvider>,
      );
    });
  };
  const title = () => container.querySelector("h1")?.textContent;
  const hash = () => window.location.hash;
  const goto = async (h: string) => {
    await act(async () => {
      window.location.hash = h;
      // jsdom fires hashchange asynchronously.
      await new Promise((r) => setTimeout(r, 0));
    });
  };

  beforeEach(() => {
    localStorage.clear();
    window.location.hash = "";
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

  const bar = () => container.querySelector('nav[aria-label="Main"]');
  const barButton = (label: string) =>
    [...(bar()?.querySelectorAll("button") ?? [])].find((b) => b.textContent?.includes(label));

  it("shows the bottom bar on the child pages, Settings and Activity", () => {
    for (const h of [
      "#/home",
      "#/ward/c1",
      "#/ward/c1/settings",
      "#/settings",
      "#/settings/key-backup",
      "#/activity",
    ]) {
      act(() => root.unmount());
      root = createRoot(container);
      mount(h);
      expect(bar(), h).not.toBeNull();
    }
  });

  it("hides the bottom bar on the full-screen editors, set-up and the recovery screen", () => {
    for (const h of [
      "#/ward/c1/rule/budget",
      "#/ward/c1/rule/app/p1",
      "#/ward/c1/device/d1",
      "#/ward/c1/add-device",
      "#/setup",
    ]) {
      act(() => root.unmount());
      root = createRoot(container);
      mount(h);
      expect(bar(), h).toBeNull();
    }
    // The editor pages sit flush: no bar means no offset for their save bars.
    expect(container.querySelector(".has-bottom-bar")).toBeNull();
  });

  it("the bar marks the current page and takes the guardian to a child", async () => {
    mount("#/settings", twoChildren());
    expect(barButton("Settings")?.getAttribute("aria-current")).toBe("page");
    expect(container.querySelector(".has-bottom-bar")).not.toBeNull();
    await act(async () => {
      barButton("Ella")!.click();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(hash()).toBe("#/ward/c2");
    expect(barButton("Ella")?.getAttribute("aria-current")).toBe("page");
    expect(barButton("Settings")?.getAttribute("aria-current")).toBeNull();
  });

  it("with children there is no Home: the bar has none, and the app opens on the first child", () => {
    for (const h of ["", "#/", "#/home"]) {
      act(() => root.unmount());
      root = createRoot(container);
      mount(h, twoChildren());
      expect(hash(), h).toBe("#/ward/c1");
      expect(title(), h).toBe("Sam");
      expect(container.querySelector('button[aria-label="Back"]'), h).toBeNull();
      expect(barButton("Home"), h).toBeUndefined();
      expect(barButton("Sam")?.getAttribute("aria-current"), h).toBe("page");
      expect(barButton("Settings"), h).toBeTruthy();
    }
  });

  it("opening on the first child adds no history entry", () => {
    window.location.hash = "#/home";
    const before = window.history.length;
    mount("#/home");
    expect(hash()).toBe("#/ward/c1");
    expect(window.history.length).toBe(before);
  });

  it("with no children Home is the getting-started page: Kintrinsic, no cog, a Home button", () => {
    mount("#/home", { ...seeded([]), children: [] });
    expect(title()).toBe("Kintrinsic");
    expect(hash()).toBe("#/home");
    expect(container.querySelector('button[aria-label="Back"]')).toBeNull();
    expect(container.querySelector("header button")).toBeNull();
    expect(container.textContent).toContain("Welcome to Kintrinsic");
    expect(barButton("Home")?.getAttribute("aria-current")).toBe("page");
    expect(barButton("Settings")).toBeTruthy();
    // Nothing a child's page holds: no waiting asks, no child cards.
    expect(container.querySelector("#waiting")).toBeNull();
    expect(container.textContent).not.toContain("Recent activity");
  });

  it("the family notices sit at the top of the child page, until a computer is connected", () => {
    const bare: CharterState = { ...seeded([]), children: [{ ...SAM, devices: [] }] };
    mount("#/ward/c1", bare);
    const main = container.querySelector("main")!;
    expect(main.textContent).toContain("Welcome to Kintrinsic");
    expect(main.textContent!.indexOf("Welcome to Kintrinsic")).toBeLessThan(main.textContent!.indexOf("Now"));
    act(() => root.unmount());
    root = createRoot(container);
    mount("#/ward/c1");
    expect(container.querySelector("main")!.textContent).not.toContain("Welcome to Kintrinsic");
  });

  it("Settings has a Recent activity row, and only when there are children", async () => {
    mount("#/settings");
    const row = (): HTMLButtonElement | undefined =>
      [...container.querySelectorAll<HTMLButtonElement>("main button")].find((b) =>
        b.textContent?.startsWith("Recent activity"),
      );
    await act(async () => {
      row()!.click();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(hash()).toBe("#/activity");
    expect(title()).toBe("Activity");
    act(() => root.unmount());
    root = createRoot(container);
    mount("#/settings", { ...seeded([]), children: [] });
    expect(row()).toBeUndefined();
  });

  it("Settings in the bottom bar opens Settings, with no back arrow", async () => {
    mount("#/ward/c1");
    await act(async () => {
      (barButton("Settings") as HTMLButtonElement).click();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(hash()).toBe("#/settings");
    expect(title()).toBe("Settings");
    expect(container.querySelector('button[aria-label="Back"]')).toBeNull();
    expect(barButton("Settings")?.getAttribute("aria-current")).toBe("page");
  });

  it("#/ward/<id> is the child page: the daily things, no rules or devices, no back arrow", () => {
    mount("#/ward/c1");
    expect(title()).toBe("Sam");
    expect(container.querySelector('button[aria-label="Back"]')).toBeNull();
    for (const id of ["now", "waiting", "week"]) {
      expect(container.querySelector(`#${id}`), id).not.toBeNull();
    }
    for (const id of ["rules", "devices", "manage"]) {
      expect(container.querySelector(`#${id}`), id).toBeNull();
    }
    expect(container.textContent).not.toContain("Daily schedule");
    expect(container.textContent).toContain("Rules, devices, name");
  });

  it("the child page's cog and its bottom row both open that child's settings", async () => {
    mount("#/ward/c1");
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('header button[aria-label="Sam’s settings"]')!
        .click();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(hash()).toBe("#/ward/c1/settings");
    expect(title()).toBe("Sam’s settings");
    await goto("#/ward/c1");
    const row = [...container.querySelectorAll("main button")].find((b) =>
      b.textContent?.includes("Rules, devices, name"),
    ) as HTMLButtonElement;
    await act(async () => {
      row.click();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(hash()).toBe("#/ward/c1/settings");
  });

  it("#/ward/<id>/settings has Rules, Devices, the device settings and Manage; no back arrow; Sam is current", () => {
    mount("#/ward/c1/settings");
    expect(title()).toBe("Sam’s settings");
    expect(container.querySelector('button[aria-label="Back"]')).toBeNull();
    expect(barButton("Sam")?.getAttribute("aria-current")).toBe("page");
    const ids = ["rules", "devices", "manage"];
    for (const id of ids) expect(container.querySelector(`#${id}`), id).not.toBeNull();
    // In order: Rules, Devices, ..., Manage.
    const order = [...container.querySelectorAll("main section[id]")].map((e) => e.id);
    expect(order.filter((id) => ids.includes(id))).toEqual(ids);
    // Rules: three groups, one row per rule, each with a summary.
    for (const t of ["Time", "Apps & web", "Safety", "Daily schedule", "Time limit", "Hotspot"]) {
      expect(container.textContent).toContain(t);
    }
    // Each device row opens that device's own page.
    expect(container.textContent).toContain("Sam's laptop");
  });

  it("#/approvals opens the first child with an ask waiting, tidied to #/ward/<id>", () => {
    mount("#/approvals", twoChildren());
    expect(hash()).toBe("#/ward/c2");
    expect(title()).toBe("Ella");
    expect(container.querySelector("#waiting")?.textContent).toContain("Ella's laptop asks for 15 more minutes");
    expect(barButton("Ella")?.getAttribute("aria-current")).toBe("page");
  });

  it("#/approvals with nothing waiting opens the first child", () => {
    mount("#/approvals", { ...twoChildren(), requests: [] });
    expect(hash()).toBe("#/ward/c1");
    expect(title()).toBe("Sam");
    expect(container.querySelector("#waiting")).toBeNull();
  });

  it("#/approvals/<id> becomes #/ward/<id>, in place, on the child's page", () => {
    // Arriving at the address adds its entry; tidying it must not add another.
    window.location.hash = "#/approvals/c1";
    const before = window.history.length;
    mount("#/approvals/c1");
    expect(hash()).toBe("#/ward/c1");
    expect(window.history.length).toBe(before);
    expect(title()).toBe("Sam");
    expect(container.querySelector("#waiting")).not.toBeNull();
  });

  it("a child who doesn't exist, or an unknown route, lands on the first child", () => {
    for (const h of ["#/ward/ghost", "#/approvals/ghost", "#/ward/ghost/rule/budget", "#/nonsense"]) {
      act(() => root.unmount());
      root = createRoot(container);
      mount(h);
      expect(title(), h).toBe("Sam");
      expect(hash(), h).toBe("#/ward/c1");
    }
  });

  it("the old tab routes still land somewhere sensible", () => {
    const landing: [string, string][] = [
      ["#/activity", "Activity"],
      ["#/activity/c1", "Activity"],
      ["#/limits", "Sam’s settings"],
      ["#/limits/c1", "Sam’s settings"],
      ["#/family", "Settings"],
      ["#/settings/anything", "Settings"],
    ];
    for (const [h, expected] of landing) {
      act(() => root.unmount());
      root = createRoot(container);
      mount(h);
      expect(title(), h).toBe(expected);
      // Top-level pages have no back arrow; Activity is one level down.
      const topLevel = expected !== "Activity";
      expect(container.querySelector('button[aria-label="Back"]') === null, h).toBe(topLevel);
    }
  });

  it("#/limits/<id> becomes #/ward/<id>/settings in place, on the child's settings", () => {
    window.location.hash = "#/limits/c1";
    const before = window.history.length;
    mount("#/limits/c1");
    expect(hash()).toBe("#/ward/c1/settings");
    expect(window.history.length).toBe(before);
    expect(title()).toBe("Sam’s settings");
    expect(container.querySelector("#rules")).not.toBeNull();
  });

  it("#/limits is the first child's settings, or Home when there are none", () => {
    mount("#/limits");
    expect(hash()).toBe("#/ward/c1/settings");
    expect(title()).toBe("Sam’s settings");
    act(() => root.unmount());
    root = createRoot(container);
    mount("#/limits", { ...seeded(), children: [] });
    expect(hash()).toBe("#/home");
    expect(title()).toBe("Kintrinsic");
  });

  it("titles the rule editor by the rule and the device page by the device", () => {
    mount("#/ward/c1/rule/budget");
    expect(title()).toBe("Time limit");
    act(() => root.unmount());
    root = createRoot(container);
    mount("#/ward/c1/device/d1");
    expect(title()).toBe("Sam's laptop");
  });

  it("Back goes to the first child when the guardian arrived by a deep link", async () => {
    const back = vi.spyOn(window.history, "back");
    mount("#/ward/c1/device/d1");
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Back"]')!.click();
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(back).not.toHaveBeenCalled();
    expect(hash()).toBe("#/ward/c1");
    expect(title()).toBe("Sam");
    back.mockRestore();
  });

  it("Back uses the browser's history once the guardian has moved around", async () => {
    const back = vi.spyOn(window.history, "back").mockImplementation(() => {});
    mount("#/home");
    await goto("#/ward/c1/device/d1");
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Back"]')!.click();
    });
    expect(back).toHaveBeenCalledTimes(1);
    back.mockRestore();
  });
});
