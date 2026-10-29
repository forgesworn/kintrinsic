import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import Home from "./Home";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child, ChildRequest } from "../domain/types";

const dev = (id: string, label: string) => ({
  id,
  label,
  platform: "linux" as const,
  pairing: "paired" as const,
  devicePubkey: id.repeat(64).slice(0, 64),
});
const SAM: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [dev("a", "Sam's laptop")],
  policies: [{ id: "p1", scope: { kind: "device" } }],
};
const ELLA: Child = { ...SAM, id: "c2", name: "Ella", devices: [dev("b", "Ella's laptop")] };

const ask = (id: string, childId: string, minutes: number, ageMs: number): ChildRequest => ({
  id,
  childId,
  requester: "device",
  kind: "time.extend",
  createdAt: Date.now() - ageMs,
  title: `asks for ${minutes} more minutes`,
  minutesRequested: minutes,
  status: "pending",
});

describe("Home", () => {
  let container: HTMLDivElement;
  let root: Root;

  const mount = (state: Partial<CharterState>) => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [SAM, ELLA],
        requests: [],
        activity: [],
        signer: { connected: true, kind: "none", autoSign: false },
        ...state,
      }),
    );
    act(() => {
      root.render(
        <CharterProvider>
          <Home />
        </CharterProvider>,
      );
    });
  };
  const buttons = () => [...container.querySelectorAll("button")].map((b) => b.textContent?.trim());

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

  it("is the getting-started page: the welcome card, with no children yet", () => {
    mount({ children: [] });
    expect(container.textContent).toContain("Welcome to Kintrinsic");
    expect(buttons()).toContain("Open the setup guide");
  });

  it("holds no child cards, waiting asks or activity link, whatever the family", () => {
    mount({ requests: [ask("r", "c1", 15, 60_000)] });
    expect(container.querySelector("#waiting")).toBeNull();
    expect(buttons()).not.toContain("More time");
    expect(buttons()).not.toContain("Stop now");
    expect(buttons()).not.toContain("Recent activity ›");
    expect(container.textContent).not.toContain("Sam wants");
  });
});
