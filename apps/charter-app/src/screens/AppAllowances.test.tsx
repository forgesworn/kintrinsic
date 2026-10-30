// The old "Named times" screen, folded under Free time → Apps as App allowances:
// the same editor and behaviour, with the word "time" kept for windows. This
// guards the vocabulary (nothing on the screen says "named time"), the three
// sections, and the way to it (it is listed with the apps, not with the hours).
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import RuleEditor, { ChildRules } from "./RuleEditor";
import { SECTION_TITLE } from "./Limits";
import { allowanceCopy } from "./allowanceCopy";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child } from "../domain/types";

const SAM: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) }],
  policies: [
    {
      id: "p1",
      scope: { kind: "device" },
      learning: {
        enabled: true,
        apps: [{ id: "khan-academy", label: "Khan Academy", kind: "site", url: "https://www.khanacademy.org/", domains: ["khanacademy.org"] }],
      },
      buckets: { enabled: true, tz: "UTC", buckets: [{ id: "play", label: "Play", apps: ["minecraft"], dailyMinutes: 60 }] },
      apps: { enabled: true, posture: "blocklist", blocked: ["tiktok"], allowed: [], askFirst: ["tiktok"] },
      freeGroups: [{ id: "homework", label: "Homework", apps: ["khan-academy"] }],
    },
  ],
};

describe("App allowances (the folded Named times screen)", () => {
  let container: HTMLDivElement;
  let root: Root;
  const mount = (ui: JSX.Element) => {
    const state: CharterState = { children: [SAM], requests: [], activity: [], signer: { connected: true, kind: "none", autoSign: true } };
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    act(() => {
      root.render(<CharterProvider>{ui}</CharterProvider>);
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

  it("is titled App allowances, and lists in Apps & web with the apps, not under the hours", () => {
    expect(SECTION_TITLE["named-times"]).toBe("App allowances");
    mount(<ChildRules child={SAM} />);
    const groups = [...container.querySelectorAll("section")].map((s) => ({
      title: s.querySelector("h3")?.textContent,
      rows: [...s.querySelectorAll(".row-title")].map((r) => r.textContent),
    }));
    expect(groups.find((g) => g.title === "Time")!.rows).toEqual(["Daily schedule", "Time limit", "When time's up"]);
    expect(groups.find((g) => g.title === "Apps & web")!.rows).toContain("App allowances");
  });

  it("says nothing about named times, in its rows or on its page", () => {
    mount(<ChildRules child={SAM} />);
    expect(container.textContent).not.toMatch(/named[ -]times?/i);
    act(() => root.unmount());
    root = createRoot(container);
    mount(<RuleEditor childId="c1" section="named-times" />);
    expect(container.textContent).not.toMatch(/named[ -]times?/i);
  });

  it("groups under Time-free, Counted and Ask first", () => {
    mount(<RuleEditor childId="c1" section="named-times" />);
    const heads = [...container.querySelectorAll("h3.rule-group-title")].map((h) => h.textContent);
    expect(heads).toEqual(["Time-free", "Counted", "Ask first"]);
    const options = [...container.querySelectorAll("button")].map((b) => b.textContent?.trim());
    expect(options).toEqual(expect.arrayContaining(["Time-free", "Counted", "Ask first", "+ Add a group"]));
    expect(options).not.toContain("On request");
    expect(options).not.toContain("+ Named time");
    expect(container.textContent).toContain("app allowances inside Free time");
  });

  it("the domain's own refusals are reworded to match", () => {
    expect(allowanceCopy("Give every named time a name — an unnamed one can’t be sent to the device.")).toBe(
      "Give every group a name — an unnamed one can’t be sent to the device.",
    );
    expect(allowanceCopy("“Play” needs a daily or weekly limit — add one, or switch it to Free or On request.")).toBe(
      "“Play” needs a daily or weekly limit — add one, or switch it to Time-free or Ask first.",
    );
    expect(allowanceCopy("Up to 12 counted named times — combine or remove one before saving.")).toBe(
      "Up to 12 counted groups — combine or remove one before saving.",
    );
  });
});
