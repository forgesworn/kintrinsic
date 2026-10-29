import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import Settings from "./Settings";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child } from "../domain/types";

const SAM: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [],
  policies: [],
};

describe("Settings", () => {
  let container: HTMLDivElement;
  let root: Root;

  const mount = (sub: string, state: Partial<CharterState> = {}) => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [SAM],
        requests: [],
        activity: [],
        signer: { connected: false, kind: "local", autoSign: false },
        ...state,
      }),
    );
    act(() => {
      root.render(
        <CharterProvider>
          <Settings sub={sub} />
        </CharterProvider>,
      );
    });
  };
  const text = () => container.textContent ?? "";
  const buttons = () => [...container.querySelectorAll("button")].map((b) => b.textContent?.trim());

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

  it("lists the children, with the way to add one, then approval, help", () => {
    mount("");
    const headings = [...container.querySelectorAll(".section-label")].map((e) => e.textContent);
    expect(headings.slice(0, 3)).toEqual(["Children and family devices", "Parent approval", "Help"]);
    expect(container.querySelector('button[aria-label="Sam"]')).not.toBeNull();
    expect(buttons()).toContain("Add a child");
    expect(text()).toContain("Setup guide");
  });

  it("has a note on the order, and ↑ ↓ buttons that reorder the children, disabled at the ends", () => {
    const ELLA: Child = { ...SAM, id: "c2", name: "Ella" };
    mount("", { children: [SAM, ELLA] });
    expect(text()).toContain("The order here is the order of the buttons at the bottom of the screen.");
    const names = () =>
      [...container.querySelectorAll<HTMLButtonElement>(".child-order-row .child-order-link")].map(
        (b) => b.getAttribute("aria-label"),
      );
    const btn = (label: string) =>
      container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
    expect(names()).toEqual(["Sam", "Ella"]);
    expect(btn("Move Sam up").disabled).toBe(true);
    expect(btn("Move Ella down").disabled).toBe(true);
    expect(btn("Move Sam down").disabled).toBe(false);
    act(() => btn("Move Sam down").click());
    expect(names()).toEqual(["Ella", "Sam"]);
    expect(btn("Move Ella up").disabled).toBe(true);
    expect(btn("Move Sam down").disabled).toBe(true);
    act(() => btn("Move Sam up").click());
    expect(names()).toEqual(["Sam", "Ella"]);
    // The order is kept.
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY)!).children.map((c: Child) => c.id)).toEqual([
      "c1",
      "c2",
    ]);
  });

  it("a child row opens that child's page; the setup guide row opens the guide", () => {
    mount("");
    act(() => (container.querySelector('button[aria-label="Sam"]') as HTMLButtonElement).click());
    expect(window.location.hash).toBe("#/ward/c1");
    act(() => [...container.querySelectorAll("button")].find((b) => b.textContent?.includes("Setup guide"))!.click());
    expect(window.location.hash).toBe("#/setup");
  });

  it("parent approval off: offers to turn it on, with no key backup or auto-sign switch", () => {
    mount("");
    expect(text()).toContain("Turn on parent approval");
    expect(text()).not.toContain("Turn off parent approval");
    expect(container.querySelector("#key-backup")).toBeNull();
    expect(container.querySelector('[role="switch"]')).toBeNull();
  });

  it("parent approval on: shows the switch, turn-off and the key backup section", () => {
    mount("", { signer: { connected: true, kind: "local", autoSign: true } });
    expect(text()).toContain("Turn off parent approval");
    expect(text()).not.toContain("Turn on parent approval");
    expect(container.querySelector('[role="switch"]')?.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector("#key-backup")).not.toBeNull();
  });

  it("add-child opens the form", () => {
    mount("add-child");
    expect(container.querySelector("#new-child-name")).not.toBeNull();
  });

  it("with no child, the form is open and says to add the first", () => {
    mount("", { children: [] });
    expect(text()).toContain("Add your first child");
    expect(container.querySelector("#new-child-name")).not.toBeNull();
  });
});
