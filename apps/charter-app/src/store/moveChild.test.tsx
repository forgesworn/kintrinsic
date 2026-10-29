// moveChild: the family's order, which is the order of the bottom bar. It swaps
// a child with their neighbour, does nothing at the ends, and persists with the
// rest of the household. Guardian-side display only: nothing is signed or sent.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import { CharterProvider, STORAGE_KEY, useCharter, type CharterContextValue } from "./store";
import type { Child } from "../domain/types";

const kid = (id: string): Child => ({
  id,
  name: id.toUpperCase(),
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [],
  policies: [],
});

describe("moveChild", () => {
  let container: HTMLDivElement;
  let root: Root;
  let store: CharterContextValue;

  function Probe() {
    store = useCharter();
    return null;
  }

  const mount = (ids: string[]) => {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: ids.map(kid),
        requests: [],
        activity: [],
        signer: { connected: false, kind: "local", autoSign: false },
      }),
    );
    act(() => {
      root.render(
        <CharterProvider>
          <Probe />
        </CharterProvider>,
      );
    });
  };
  const order = () => store.state.children.map((c) => c.id);
  const persisted = () =>
    (JSON.parse(localStorage.getItem(STORAGE_KEY)!).children as Child[]).map((c) => c.id);

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

  it("swaps a child with the neighbour in either direction", () => {
    mount(["a", "b", "c"]);
    act(() => store.moveChild("b", 1));
    expect(order()).toEqual(["a", "c", "b"]);
    act(() => store.moveChild("b", -1));
    act(() => store.moveChild("b", -1));
    expect(order()).toEqual(["b", "a", "c"]);
  });

  it("does nothing at the ends, or for a child who isn't there", () => {
    mount(["a", "b", "c"]);
    const before = store.state;
    act(() => store.moveChild("a", -1));
    act(() => store.moveChild("c", 1));
    act(() => store.moveChild("ghost", 1));
    expect(order()).toEqual(["a", "b", "c"]);
    expect(store.state).toBe(before);
  });

  it("persists the order", () => {
    mount(["a", "b", "c"]);
    act(() => store.moveChild("a", 1));
    expect(persisted()).toEqual(["b", "a", "c"]);
  });

  it("leaves everything else about the children alone", () => {
    mount(["a", "b"]);
    const [a, b] = store.state.children;
    act(() => store.moveChild("a", 1));
    expect(store.state.children).toEqual([b, a]);
  });
});
