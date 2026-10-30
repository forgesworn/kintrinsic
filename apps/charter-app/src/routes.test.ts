import { describe, expect, it } from "vitest";
import { resolveRoute, waitingChildIds } from "./routes";
import type { Child, ChildRequest } from "./domain/types";

const SAM: Child = {
  id: "sam",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired" }],
  policies: [{ id: "p1", scope: { kind: "device" } }],
};
const ELLA: Child = { ...SAM, id: "ella s", name: "Ella", devices: [] };
const FAMILY = [SAM, ELLA];

describe("route table", () => {
  it("lands the empty route and #/home on the first child, in the family's order", () => {
    const first = { kind: "ward", childId: "sam", canonical: "ward/sam" };
    expect(resolveRoute("", FAMILY)).toEqual(first);
    expect(resolveRoute("home", FAMILY)).toEqual(first);
    expect(resolveRoute("", [ELLA, SAM])).toEqual({
      kind: "ward",
      childId: "ella s",
      canonical: "ward/ella%20s",
    });
  });

  it("with no children there is only Home", () => {
    for (const r of ["", "home", "nonsense", "approvals", "approvals/sam", "ward/sam", "ward/sam/settings"]) {
      expect(resolveRoute(r, []), r).toEqual({ kind: "home" });
    }
  });

  it("opens a child's page, and decodes an id that needed escaping", () => {
    expect(resolveRoute("ward/sam", FAMILY)).toEqual({ kind: "ward", childId: "sam" });
    expect(resolveRoute("ward/ella%20s", FAMILY)).toEqual({ kind: "ward", childId: "ella s" });
  });

  it("opens a rule editor, a device page and the add-device page under a child", () => {
    expect(resolveRoute("ward/sam/rule/budget", FAMILY)).toEqual({
      kind: "rule",
      childId: "sam",
      section: "budget",
    });
    expect(resolveRoute("ward/sam/device/d1", FAMILY)).toEqual({
      kind: "device",
      childId: "sam",
      deviceId: "d1",
    });
    expect(resolveRoute("ward/sam/settings", FAMILY)).toEqual({ kind: "ward-settings", childId: "sam" });
    expect(resolveRoute("ward/ella%20s/settings", FAMILY)).toEqual({
      kind: "ward-settings",
      childId: "ella s",
    });
    expect(resolveRoute("ward/sam/add-device", FAMILY)).toEqual({
      kind: "add-device",
      childId: "sam",
    });
  });

  it("falls back to the child's page for an unknown rule or device", () => {
    expect(resolveRoute("ward/sam/rule/nonsense", FAMILY)).toEqual({ kind: "ward", childId: "sam" });
    expect(resolveRoute("ward/sam/device/gone", FAMILY)).toEqual({ kind: "ward", childId: "sam" });
  });

  it("sends a child who doesn't exist to the first child, whatever the page", () => {
    for (const r of [
      "ward/ghost",
      "ward/ghost/rule/budget",
      "ward/ghost/device/d1",
      "ward/ghost/add-device",
      "ward/ghost/settings",
      "approvals/ghost",
    ]) {
      expect(resolveRoute(r, FAMILY), r).toEqual({ kind: "ward", childId: "sam", canonical: "ward/sam" });
    }
  });

  it("#/approvals is the first child with something waiting, scrolled to Waiting for you", () => {
    expect(resolveRoute("approvals", FAMILY, new Set(["ella s"]))).toEqual({
      kind: "ward",
      childId: "ella s",
      scroll: "waiting",
      canonical: "ward/ella%20s",
    });
    // Both waiting: the first in the family's order.
    expect(resolveRoute("approvals", FAMILY, new Set(["ella s", "sam"]))).toMatchObject({ childId: "sam" });
    expect(resolveRoute("approvals", [ELLA, SAM], new Set(["ella s", "sam"]))).toMatchObject({
      childId: "ella s",
    });
  });

  it("#/approvals with nothing waiting is the first child", () => {
    expect(resolveRoute("approvals", FAMILY)).toEqual({
      kind: "ward",
      childId: "sam",
      scroll: "waiting",
      canonical: "ward/sam",
    });
  });

  it("#/approvals/<childId> is that child's page at Waiting, with the address tidied", () => {
    expect(resolveRoute("approvals/sam", FAMILY)).toEqual({
      kind: "ward",
      childId: "sam",
      scroll: "waiting",
      canonical: "ward/sam",
    });
  });

  it("keeps the activity and limits routes as full pages, with or without a child", () => {
    expect(resolveRoute("activity", FAMILY)).toEqual({ kind: "activity" });
    expect(resolveRoute("activity/sam", FAMILY)).toEqual({ kind: "activity" });
    expect(resolveRoute("limits", FAMILY)).toEqual({ kind: "limits" });
    expect(resolveRoute("limits/sam", FAMILY)).toEqual({ kind: "limits" });
  });

  it("serves settings, and the old family routes, from Settings", () => {
    expect(resolveRoute("settings", FAMILY)).toEqual({ kind: "settings", sub: "" });
    expect(resolveRoute("settings/key-backup", FAMILY)).toEqual({ kind: "settings", sub: "key-backup" });
    expect(resolveRoute("family", FAMILY)).toEqual({ kind: "settings", sub: "" });
    expect(resolveRoute("family/anything", FAMILY)).toEqual({ kind: "settings", sub: "" });
  });

  it("leaves the setup guide alone, and sends anything unknown to the first child", () => {
    expect(resolveRoute("setup", FAMILY)).toEqual({ kind: "setup" });
    expect(resolveRoute("nonsense/at/all", FAMILY)).toEqual({
      kind: "ward",
      childId: "sam",
      canonical: "ward/sam",
    });
  });

  it("lists the children with an ask waiting, and only the pending ones", () => {
    const ask = (childId: string, status: ChildRequest["status"]): ChildRequest => ({
      id: `${childId}-${status}`,
      childId,
      requester: "device",
      kind: "time.extend",
      createdAt: 0,
      title: "asks",
      status,
    });
    const ids = waitingChildIds([ask("sam", "approved"), ask("ella s", "pending"), ask("ella s", "pending")]);
    expect([...ids]).toEqual(["ella s"]);
  });
});

describe("a time's editor", () => {
  const kids = [{ id: "c1", name: "Sam", color: "#000", dependantPubkey: null, devices: [], policies: [] }] as never;

  it("resolves an existing time and a new one", async () => {
    const { resolveRoute } = await import("./routes");
    expect(resolveRoute("ward/c1/time/learning", kids)).toEqual({ kind: "time", childId: "c1", timeId: "learning" });
    expect(resolveRoute("ward/c1/time/new/learning", kids)).toEqual({
      kind: "time",
      childId: "c1",
      template: "learning",
      name: undefined,
    });
    expect(resolveRoute("ward/c1/time/new/custom/Reading%20time", kids)).toEqual({
      kind: "time",
      childId: "c1",
      template: "custom",
      name: "Reading time",
    });
  });

  it("a malformed one lands on the child's page, not a blank", async () => {
    const { resolveRoute } = await import("./routes");
    expect(resolveRoute("ward/c1/time", kids)).toEqual({ kind: "ward", childId: "c1" });
    expect(resolveRoute("ward/c1/time/new/sideways", kids)).toEqual({ kind: "ward", childId: "c1" });
  });

  it("builds the paths it parses", async () => {
    const { newTimePath, timePath } = await import("./routes");
    expect(timePath("c1", "learning")).toBe("ward/c1/time/learning");
    expect(newTimePath("c1", "learning")).toBe("ward/c1/time/new/learning");
    expect(newTimePath("c1", "custom", "Reading time")).toBe("ward/c1/time/new/custom/Reading%20time");
  });
});
