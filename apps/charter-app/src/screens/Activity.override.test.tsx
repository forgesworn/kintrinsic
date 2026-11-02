// An emergency unlock reads with its exact time, on the Activity list and on
// the child page (which reuses the same row).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import ChildPage from "./ChildPage";
import { exactWhen } from "./Activity";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child } from "../domain/types";

// Fixed in local time so the assertions do not depend on the machine's zone.
// Sat 31 Oct 2026, 18:42 — and "now" shortly after, on the same day.
const PRESSED = new Date(2026, 9, 31, 18, 42).getTime();
const SAME_DAY = new Date(2026, 9, 31, 18, 47).getTime();
const TWO_DAYS_ON = new Date(2026, 10, 2, 9, 0).getTime();
const NEXT_YEAR = new Date(2027, 0, 5, 9, 0).getTime();

// 18:42 in a 24-hour locale, 6:42 PM in a 12-hour one.
const TIME = /(18:42|0?6:42)/;

describe("exactWhen", () => {
  it("shows just the time for something that happened today", () => {
    const s = exactWhen(PRESSED, SAME_DAY);
    expect(s).toMatch(TIME);
    expect(s).not.toMatch(/[A-Za-z]{3}/); // no weekday or month
  });

  it("adds the day when it was not today", () => {
    const s = exactWhen(PRESSED, TWO_DAYS_ON);
    expect(s).toMatch(TIME);
    expect(s).toMatch(/Sat/);
    expect(s).toMatch(/31/);
    expect(s).toMatch(/Oct/);
    expect(s).not.toMatch(/2026/);
  });

  it("adds the year when it was a different year", () => {
    expect(exactWhen(PRESSED, NEXT_YEAR)).toMatch(/2026/);
  });
});

const CHILD: Child = {
  id: "c1",
  name: "Robin",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: "d1", label: "Laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) }],
  policies: [{ id: "p1", scope: { kind: "device" } }],
};

describe("the child page", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    localStorage.clear();
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    localStorage.clear();
  });

  function mount(activity: CharterState["activity"]) {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [CHILD],
        requests: [],
        activity,
        signer: { connected: false, kind: "none", autoSign: false },
      }),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <ChildPage child={CHILD} />
        </CharterProvider>,
      );
    });
  }

  it("shows an emergency unlock with the exact time it was pressed", () => {
    const pressed = Date.now() - 5 * 60_000;
    mount([
      {
        id: "a1",
        childId: "c1",
        ts: pressed,
        outcome: "override",
        summary: "Opened their phone on Laptop for 5 minutes",
        auditKey: "k",
      },
    ]);
    const row = Array.from(container.querySelectorAll("li.row")).find((li) =>
      li.textContent?.includes("Emergency unlock"),
    );
    expect(row).toBeTruthy();
    const time = row!.querySelector("time")!;
    expect(time.getAttribute("datetime")).toBe(new Date(pressed).toISOString());
    expect(time.textContent).toContain(exactWhen(pressed, Date.now()));
    expect(time.textContent).toContain(" · ");
    expect(time.textContent).toMatch(/\dm ago|Just now/);
  });

  it("leaves other rows with the relative time alone", () => {
    mount([{ id: "a2", childId: "c1", ts: Date.now() - 5 * 60_000, outcome: "enacted", summary: "You set up Laptop" }]);
    const time = container.querySelector("li.row time")!;
    expect(time.textContent).toMatch(/^\dm ago$/);
  });
});
