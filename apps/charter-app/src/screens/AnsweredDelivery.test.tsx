// The "what became of your answer" lines: the whole family's by default, one
// child's when the child page asks for it.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import AnsweredDelivery from "./AnsweredDelivery";
import { CharterProvider, STORAGE_KEY } from "../store/store";
import type { Child, ChildRequest } from "../domain/types";
import type { LocalWrapRecord } from "../store/localDelivery";

const NOW = Date.now();
const kid = (id: string, name: string, machine: string): Child => ({
  id,
  name,
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: `${id}-d`, label: `${name}'s laptop`, platform: "linux", pairing: "paired", devicePubkey: machine }],
  policies: [{ id: `${id}-p`, scope: { kind: "device" } }],
});
const SAM = kid("c1", "Sam", "a".repeat(64));
const ELLA = kid("c2", "Ella", "b".repeat(64));

const answered = (child: Child, reqId: string): ChildRequest => ({
  id: `req-${child.id}`,
  childId: child.id,
  requester: "device",
  deviceId: `${child.id}-d`,
  kind: "time.extend",
  createdAt: NOW - 60_000,
  title: `${child.name}'s laptop asks`,
  status: "approved",
  reqId,
});
const delivered = (child: Child, reqId: string): LocalWrapRecord => ({
  wrap: `w-${child.id}`,
  machine: child.devices[0].devicePubkey!,
  deviceId: child.devices[0].id,
  childId: child.id,
  kind: "grant",
  reqId,
  at: NOW - 30_000,
  state: "applied",
});

describe("AnsweredDelivery", () => {
  let container: HTMLDivElement;
  let root: Root;

  const mount = (childId?: string) => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [SAM, ELLA],
        requests: [answered(SAM, "1".repeat(64)), answered(ELLA, "2".repeat(64))],
        localDeliveries: [delivered(SAM, "1".repeat(64)), delivered(ELLA, "2".repeat(64))],
        activity: [],
        signer: { connected: true, kind: "none", autoSign: false },
      }),
    );
    act(() => {
      root.render(
        <CharterProvider>
          <AnsweredDelivery childId={childId} />
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

  it("with no child, shows every child's answers", () => {
    mount();
    expect(container.textContent).toContain("Applied on Sam's laptop");
    expect(container.textContent).toContain("Applied on Ella's laptop");
  });

  it("with a child, shows only that child's answers", () => {
    mount("c2");
    expect(container.textContent).toContain("Applied on Ella's laptop");
    expect(container.textContent).not.toContain("Sam's laptop");
  });

  it("with a child who has none, renders nothing", () => {
    mount("ghost");
    expect(container.querySelector("section")).toBeNull();
  });
});
