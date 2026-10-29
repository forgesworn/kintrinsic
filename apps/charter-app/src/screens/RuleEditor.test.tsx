// The rule editors as pages: each section alone, one Save model, the singled-out
// app pages, the unsaved-changes guard and the child page's Rules list. The
// store's own writes are spied; everything else is the real provider.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";

const spies = vi.hoisted(() => ({
  savePolicy: vi.fn(),
  saveAppRule: vi.fn(),
  removePolicy: vi.fn(),
  addAppLimit: vi.fn(),
}));

vi.mock("../store/store", async (importOriginal) => {
  const real = await importOriginal<typeof import("../store/store")>();
  return { ...real, useCharter: () => ({ ...real.useCharter(), ...spies }) };
});

import App from "../App";
import RuleEditor, { AppRulePage, ChildRules } from "./RuleEditor";
import { SECTION_TITLE, type SectionId } from "./Limits";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child } from "../domain/types";

const SAM: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [
    { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) },
  ],
  policies: [
    {
      id: "p1",
      scope: { kind: "device" },
      schedule: { tz: "UTC", weekly: { mon: [{ start: "16:00", end: "18:00" }] } },
    },
    { id: "app1", scope: { kind: "app", appId: "app_roblox", label: "Roblox" }, blocked: true },
    { id: "app2", scope: { kind: "app", appId: "app_youtube", label: "YouTube" } },
  ],
};

const ALL_IDS = Object.keys(SECTION_TITLE) as SectionId[];

describe("rule editors as pages", () => {
  let container: HTMLDivElement;
  let root: Root;

  const seed = (children: Child[] = [SAM]) => {
    const state: CharterState = {
      children,
      requests: [],
      activity: [],
      signer: { connected: true, kind: "none", autoSign: true },
    };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  };
  const mount = (ui: JSX.Element, children?: Child[]) => {
    seed(children);
    act(() => {
      root.render(<CharterProvider>{ui}</CharterProvider>);
    });
  };
  const text = () => container.textContent ?? "";
  const byLabel = (label: string) =>
    container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
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

  beforeEach(() => {
    localStorage.clear();
    window.location.hash = "";
    spies.savePolicy.mockReset().mockResolvedValue({ outcome: "sent" });
    spies.saveAppRule.mockReset().mockResolvedValue({ outcome: "sent" });
    spies.removePolicy.mockReset();
    spies.addAppLimit.mockReset().mockReturnValue({ id: "newapp", scope: { kind: "app", appId: "x", label: "X" } });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
    window.location.hash = "";
    vi.restoreAllMocks();
  });

  describe("RuleEditor", () => {
    it("renders each section on its own: no tabs, no accordion, a Save bar", () => {
      const seen = new Map<string, string>();
      for (const id of ALL_IDS) {
        act(() => root.unmount());
        root = createRoot(container);
        mount(<RuleEditor childId="c1" section={id} />);
        expect(container.querySelector('[role="tablist"]'), id).toBeNull();
        expect(container.querySelector(".acc, .seg"), id).toBeNull();
        expect(container.querySelector(".rule-bar"), id).not.toBeNull();
        expect(button("Cancel"), id).toBeTruthy();
        expect(button("Save").disabled, id).toBe(true);
        seen.set(id, container.innerHTML);
      }
      // Nine different pages, not one page nine times.
      expect(new Set(seen.values()).size).toBe(ALL_IDS.length);
    });

    it("shows only the named section's controls", () => {
      const marker: Partial<Record<SectionId, string>> = {
        schedule: '[aria-label="Off unless I open it"]',
        budget: '[aria-label="Limit time each day"]',
        listening: 'input[name="listening-mode"]',
        web: '[aria-label="Filter websites"]',
        apps: '[aria-label="Control apps"]',
        tethering: '[aria-label="Allow hotspot"]',
      };
      for (const [id, sel] of Object.entries(marker) as [SectionId, string][]) {
        act(() => root.unmount());
        root = createRoot(container);
        mount(<RuleEditor childId="c1" section={id} />);
        expect(container.querySelector(sel), `${id} shows its own control`).not.toBeNull();
        for (const other of Object.values(marker)) {
          if (other !== sel) expect(container.querySelector(other!), `${id} also shows ${other}`).toBeNull();
        }
      }
    });

    it("Save for the schedule calls savePolicy once, with only what changed, then goes to the child page", async () => {
      window.location.hash = "/ward/c1/rule/schedule";
      mount(<RuleEditor childId="c1" section="schedule" />);
      expect(button("Save").disabled).toBe(true);
      await click(byLabel("Pause the schedule")!);
      expect(button("Save").disabled).toBe(false);
      await click(button("Save"));

      expect(spies.savePolicy).toHaveBeenCalledTimes(1);
      const [childId, policyId, payload] = spies.savePolicy.mock.calls[0];
      expect([childId, policyId]).toEqual(["c1", "p1"]);
      expect(payload.schedule).toMatchObject({ paused: true });
      for (const k of ["budget", "web", "apps", "lifeline", "tethering", "listening", "alwaysAvailable", "learning", "buckets", "freeGroups"]) {
        expect(payload[k], k).toBeUndefined();
      }
      expect(payload.deviceOverrides).toEqual({});
      expect(window.location.hash).toBe("#/ward/c1/settings");
    });

    it("Save for websites calls savePolicy once with the web draft", async () => {
      mount(<RuleEditor childId="c1" section="web" />);
      await click(byLabel("Filter websites")!);
      await click(button("Save"));

      expect(spies.savePolicy).toHaveBeenCalledTimes(1);
      const payload = spies.savePolicy.mock.calls[0][2];
      expect(payload.web).toMatchObject({ enabled: true });
      expect(payload.schedule).toBeUndefined();
      expect(payload.deviceOverrides).toEqual({});
    });

    it("Cancel does not save, and goes back to the child page", async () => {
      window.location.hash = "/ward/c1/rule/web";
      mount(<RuleEditor childId="c1" section="web" />);
      await click(byLabel("Filter websites")!);
      await click(button("Cancel"));
      expect(spies.savePolicy).not.toHaveBeenCalled();
      expect(window.location.hash).toBe("#/ward/c1/settings");
    });

    it("a save the carrier is holding stays on the page and shows the warning", async () => {
      spies.savePolicy.mockResolvedValue({ outcome: "queued", devices: ["Sam's laptop"] });
      window.location.hash = "/ward/c1/rule/web";
      mount(<RuleEditor childId="c1" section="web" />);
      await click(byLabel("Filter websites")!);
      await click(button("Save"));
      expect(window.location.hash).toBe("#/ward/c1/rule/web");
      expect(text()).toContain("Saved here. It'll reach Sam's laptop when it's on the home network.");
    });

    it("declining the signature stays on the page with the draft intact", async () => {
      spies.savePolicy.mockResolvedValue({ outcome: "cancelled" });
      window.location.hash = "/ward/c1/rule/web";
      mount(<RuleEditor childId="c1" section="web" />);
      await click(byLabel("Filter websites")!);
      await click(button("Save"));
      expect(spies.savePolicy).toHaveBeenCalledTimes(1);
      expect(window.location.hash).toBe("#/ward/c1/rule/web");
      // Still dirty, so Save is still on offer and the edit is still there.
      expect(button("Save").disabled).toBe(false);
      expect(byLabel("Filter websites")!.getAttribute("aria-checked")).toBe("true");
    });

    it("a failed save stays put with the error", async () => {
      spies.savePolicy.mockRejectedValue(new Error("bunker timed out"));
      window.location.hash = "/ward/c1/rule/web";
      mount(<RuleEditor childId="c1" section="web" />);
      await click(byLabel("Filter websites")!);
      await click(button("Save"));
      expect(window.location.hash).toBe("#/ward/c1/rule/web");
      expect(text()).toContain("Couldn’t save these changes: bunker timed out");
    });

    it("says it will ask the signer first when the guardian confirms each change", async () => {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({
          children: [SAM],
          requests: [],
          activity: [],
          signer: { connected: true, kind: "nip46", label: "Signet", autoSign: false },
        }),
      );
      act(() => {
        root.render(
          <CharterProvider>
            <RuleEditor childId="c1" section="web" />
          </CharterProvider>,
        );
      });
      expect(text()).not.toContain("You’ll confirm");
      await click(byLabel("Filter websites")!);
      expect(container.querySelector(".rule-bar")?.textContent).toContain(
        "You’ll confirm these changes with Signet before they apply.",
      );
    });

    it("the Apps page lists the singled-out apps under their heading and adds one", async () => {
      window.location.hash = "/ward/c1/rule/apps";
      mount(<RuleEditor childId="c1" section="apps" />);
      expect(container.querySelector("h2")?.textContent).toBe("Games and apps you've singled out");
      const rows = [...container.querySelectorAll("button.row")].map((b) => b.textContent);
      expect(rows).toEqual(["RobloxBlocked›", "YouTubeAllowed›"]);

      await click(button("+ Add a game or app"));
      await click(button("+ Roblox"));
      expect(spies.addAppLimit).toHaveBeenCalledWith("c1", { appId: "app_roblox", label: "Roblox" });
      expect(window.location.hash).toBe("#/ward/c1/rule/app/newapp");
    });

    it("opens a singled-out app's own page from its row", async () => {
      window.location.hash = "/ward/c1/rule/apps";
      mount(<RuleEditor childId="c1" section="apps" />);
      await click([...container.querySelectorAll<HTMLButtonElement>("button.row")][1]);
      expect(window.location.hash).toBe("#/ward/c1/rule/app/app2");
    });
  });

  describe("AppRulePage", () => {
    it("Save calls saveAppRule once with the block and hours", async () => {
      window.location.hash = "/ward/c1/rule/app/app2";
      mount(<AppRulePage childId="c1" policyId="app2" />);
      expect(button("Save").disabled).toBe(true);
      await click(byLabel("Block YouTube")!);
      await click(button("Save"));
      expect(spies.saveAppRule).toHaveBeenCalledTimes(1);
      expect(spies.saveAppRule).toHaveBeenCalledWith("c1", "app2", { blocked: true, schedule: undefined });
      expect(window.location.hash).toBe("#/ward/c1/settings");
    });

    it("Cancel does not save", async () => {
      mount(<AppRulePage childId="c1" policyId="app1" />);
      await click(byLabel("Block Roblox")!);
      await click(button("Cancel"));
      expect(spies.saveAppRule).not.toHaveBeenCalled();
    });

    it("Remove calls removePolicy and leaves", async () => {
      window.location.hash = "/ward/c1/rule/app/app1";
      mount(<AppRulePage childId="c1" policyId="app1" />);
      await click(button("Remove Roblox"));
      expect(spies.removePolicy).toHaveBeenCalledWith("c1", "app1");
      expect(window.location.hash).toBe("#/ward/c1/settings");
    });

    it("declining the signature stays on the page with the draft intact", async () => {
      spies.saveAppRule.mockResolvedValue(undefined);
      window.location.hash = "/ward/c1/rule/app/app2";
      mount(<AppRulePage childId="c1" policyId="app2" />);
      await click(byLabel("Block YouTube")!);
      await click(button("Save"));
      expect(spies.saveAppRule).toHaveBeenCalledTimes(1);
      expect(window.location.hash).toBe("#/ward/c1/rule/app/app2");
      expect(button("Save").disabled).toBe(false);
      expect(text()).not.toContain("Couldn’t save");
    });

    it("a held save stays on the page with its warning", async () => {
      spies.saveAppRule.mockResolvedValue({ outcome: "queued", devices: ["Sam's laptop"] });
      window.location.hash = "/ward/c1/rule/app/app2";
      mount(<AppRulePage childId="c1" policyId="app2" />);
      await click(byLabel("Block YouTube")!);
      await click(button("Save"));
      expect(window.location.hash).toBe("#/ward/c1/rule/app/app2");
      expect(text()).toContain("Saved here. It'll reach Sam's laptop when it's on the home network.");
    });
  });

  describe("in the app shell", () => {
    const title = () => container.querySelector("h1")?.textContent;

    it("titles #/ward/<id>/rule/app/<policyId> by the app, and unknown policies say so", () => {
      window.location.hash = "/ward/c1/rule/app/app1";
      mount(<App />);
      expect(title()).toBe("Roblox");
      expect(button("Remove Roblox")).toBeTruthy();
    });

    // jsdom's history.back() is asynchronous and depends on earlier entries;
    // stand in for it with the navigation it would cause.
    const backGoesHome = () =>
      vi.spyOn(window.history, "back").mockImplementation(() => {
        window.location.hash = "/home";
      });

    it("asks before the back arrow drops unsaved changes, and stays if declined", async () => {
      backGoesHome();
      const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
      window.location.hash = "/ward/c1/rule/web";
      mount(<App />);
      await click(byLabel("Filter websites")!);
      await click(byLabel("Back")!);
      expect(confirm).toHaveBeenCalledTimes(1);
      expect(window.location.hash).toBe("#/ward/c1/rule/web");
      expect(title()).toBe("Websites");

      confirm.mockReturnValue(true);
      await click(byLabel("Back")!);
      expect(window.location.hash).toBe("#/ward/c1");
      expect(title()).toBe("Sam");
    });

    it("asks when the address changes under unsaved changes, and puts it back if declined", async () => {
      const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
      window.location.hash = "/ward/c1/rule/web";
      mount(<App />);
      await click(byLabel("Filter websites")!);
      await act(async () => {
        window.location.hash = "/settings";
        await new Promise((r) => setTimeout(r, 0));
      });
      expect(confirm).toHaveBeenCalledTimes(1);
      expect(window.location.hash).toBe("#/ward/c1/rule/web");
      expect(title()).toBe("Websites");
    });

    it("does not ask when nothing has changed", async () => {
      backGoesHome();
      const confirm = vi.spyOn(window, "confirm");
      window.location.hash = "/ward/c1/rule/web";
      mount(<App />);
      await click(byLabel("Back")!);
      expect(confirm).not.toHaveBeenCalled();
      expect(title()).toBe("Sam");
    });

    it("does not ask after a successful save", async () => {
      const back = backGoesHome();
      const confirm = vi.spyOn(window, "confirm");
      window.location.hash = "/ward/c1/rule/web";
      mount(<App />);
      await click(byLabel("Filter websites")!);
      await click(button("Save"));
      expect(confirm).not.toHaveBeenCalled();
      expect(spies.savePolicy).toHaveBeenCalledTimes(1);
      expect(back).toHaveBeenCalledTimes(1);
    });
  });

  describe("a save that lands after the guardian has left", () => {
    it("does not navigate back a second time", async () => {
      const back = vi.spyOn(window.history, "back").mockImplementation(() => {
        window.location.hash = "/home";
      });
      const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
      let finish!: (d: { outcome: "sent" }) => void;
      spies.savePolicy.mockImplementation(() => new Promise((resolve) => (finish = resolve)));
      window.location.hash = "/ward/c1/rule/web";
      mount(<App />);
      await click(byLabel("Filter websites")!);
      await click(button("Save"));
      expect(spies.savePolicy).toHaveBeenCalledTimes(1);

      // Leave mid-save, then it lands. Save is already pressed, so no question.
      await act(async () => {
        window.location.hash = "/settings";
        await new Promise((r) => setTimeout(r, 0));
      });
      expect(window.location.hash).toBe("#/settings");
      expect(confirm).not.toHaveBeenCalled();
      await act(async () => {
        finish({ outcome: "sent" });
        await new Promise((r) => setTimeout(r, 0));
      });
      expect(back).not.toHaveBeenCalled();
      expect(window.location.hash).toBe("#/settings");
    });
  });

  describe("ChildRules", () => {
    it("lists every rule in Time, Apps & web and Safety with its summary", () => {
      mount(<ChildRules child={SAM} />);
      const groups = [...container.querySelectorAll("section")].map((s) => s.getAttribute("aria-label"));
      expect(groups).toEqual(["Time", "Apps & web", "Safety"]);
      for (const id of ALL_IDS) expect(text()).toContain(SECTION_TITLE[id]);
      expect(text()).toContain("Allowed 1 day a week");
    });

    it("rows open their editor page", async () => {
      mount(<ChildRules child={SAM} />);
      const row = [...container.querySelectorAll<HTMLButtonElement>("button.row")].find((b) =>
        b.textContent?.startsWith("Time limit"),
      );
      await click(row!);
      expect(window.location.hash).toBe("#/ward/c1/rule/budget");
    });

    it("warns once, with a way to set up a device, when nothing is enforcing yet", async () => {
      const unpaired: Child = { ...SAM, devices: [{ ...SAM.devices[0], pairing: "unpaired", devicePubkey: undefined }] };
      mount(<ChildRules child={unpaired} />);
      expect(text()).toContain("Nothing is enforcing these yet");
      expect(text().match(/Nothing is enforcing/g)).toHaveLength(1);
      await click(button("Set up a device"));
      expect(window.location.hash).toBe("#/ward/c1/add-device");
    });

    it("says there is no charter, and does not open rows, when there is no device policy", async () => {
      const bare: Child = { ...SAM, policies: [] };
      mount(<ChildRules child={bare} />);
      expect(text()).toContain("Sam has no charter to edit yet.");
      expect(container.querySelectorAll("button.row")).toHaveLength(0);
      expect(text()).toContain("Daily schedule");
    });
  });
});
