import { describe, expect, it } from "vitest";
import { buildListening, listeningSummary } from "./Limits";
import { listeningToGrant } from "../wire/clause";
import type { ListeningPolicy } from "../domain/types";

const AT = 1_782_752_400;

describe("what happens to a story when time's up", () => {
  /**
   * The default must be the behaviour that existed before the setting did.
   * A family who never opens this section keeps exactly what they had.
   */
  it("defaults to stopping", () => {
    expect(buildListening(undefined).mode).toBe("stop");
    expect(listeningToGrant(buildListening(undefined), AT).mode).toBe("stop");
  });

  it("carries the mode and the named apps to the wire", () => {
    const p: ListeningPolicy = { mode: "continue", apps: ["com.audible.application"] };
    const g = listeningToGrant(p, AT);
    expect(g).toEqual({
      v: 1,
      issuedAt: AT,
      mode: "continue",
      apps: ["com.audible.application"],
    });
    // `continue` carries no grace — a duration there would be noise the device
    // has to decide whether to believe.
    expect(g.graceMinutes).toBeUndefined();
  });

  /**
   * The device fails CLOSED on a grace it cannot trust: out of range means
   * "stop". Shipping one would silently revoke the permission the guardian
   * just granted — a setting that reads as one thing and behaves as its
   * opposite. So clamp here rather than send it.
   */
  it("clamps a grace into the range the device will honour", () => {
    const at = (m: number) =>
      listeningToGrant({ mode: "grace", graceMinutes: m, apps: ["a"] }, AT).graceMinutes;
    expect(at(0)).toBe(1);
    expect(at(-5)).toBe(1);
    expect(at(9999)).toBe(240);
    expect(at(30)).toBe(30);
  });

  it("drops blank app entries rather than shipping them", () => {
    const g = listeningToGrant({ mode: "continue", apps: ["  ", "com.book", ""] }, AT);
    expect(g.apps).toEqual(["com.book"]);
  });

  /**
   * Naming no apps is the same as stopping, and the summary must say so —
   * otherwise a guardian reads "may keep playing" over a device where nothing
   * can.
   */
  it("tells the truth when nothing is named", () => {
    expect(listeningSummary({ mode: "continue", apps: [] })).toBe("Audio stops with the screen");
    expect(listeningSummary({ mode: "stop", apps: ["com.book"] })).toBe(
      "Audio stops with the screen",
    );
  });

  it("says which agreement is in force", () => {
    expect(listeningSummary({ mode: "continue", apps: ["com.book"] })).toBe(
      "Audio may keep playing",
    );
    expect(listeningSummary({ mode: "grace", graceMinutes: 20, apps: ["com.book"] })).toBe(
      "Audio may finish (20 min)",
    );
  });
});
