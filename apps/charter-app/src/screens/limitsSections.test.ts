import { describe, expect, it } from "vitest";
import {
  alwaysAvailableSummary,
  appsSummary,
  buildAlwaysAvailable,
  buildApps,
  lifelineSummary,
  namedTimesSummary,
  SECTION_TABS,
  SECTION_TAB_OF,
  SECTION_TITLE,
  buildLifeline,
  type SectionId,
} from "./Limits";

/**
 * Splitting nine always-open editors across three tabs is exactly the change
 * that can lose one. If a control's id stops mapping to a tab, or maps to a tab
 * that isn't rendered, it disappears from the app while every test still passes
 * and the wire code is untouched — a rule the guardian can no longer reach.
 */
describe("every control still has somewhere to live", () => {
  const ALL = Object.keys(SECTION_TITLE) as SectionId[];

  it("covers all nine controls", () => {
    expect(ALL.sort()).toEqual(
      [
        "always-available",
        "apps",
        "budget",
        "lifeline",
        "listening",
        "named-times",
        "schedule",
        "tethering",
        "web",
      ].sort(),
    );
  });

  it("puts each control on exactly one tab that is actually shown", () => {
    const shown = SECTION_TABS.map((t) => t.id);
    for (const id of ALL) {
      const tab = SECTION_TAB_OF[id];
      expect(tab, `${id} has no tab`).toBeDefined();
      expect(shown, `${id} lives on a tab nobody can open`).toContain(tab);
    }
  });

  it("leaves no tab empty", () => {
    for (const t of SECTION_TABS) {
      expect(
        ALL.filter((id) => SECTION_TAB_OF[id] === t.id).length,
        `the ${t.label} tab has nothing on it`,
      ).toBeGreaterThan(0);
    }
  });

  it("gives every control a title to show when collapsed", () => {
    for (const id of ALL) expect(SECTION_TITLE[id].trim()).not.toBe("");
  });
});

/**
 * A collapsed row is the ONLY thing a guardian sees until they open it, so its
 * summary has to be true. These three are the summaries written for this pass.
 */
describe("what a collapsed row says", () => {
  it("says nothing is named yet in the positive", () => {
    expect(namedTimesSummary([])).toBe("No named times yet");
  });

  it("counts groups by policy", () => {
    expect(
      namedTimesSummary([
        { id: "f", label: "Learning", apps: [], policy: "free" },
        { id: "c", label: "Play", apps: [], policy: "counted", dailyMinutes: 60 },
        { id: "c2", label: "Chat", apps: [], policy: "counted", dailyMinutes: 30 },
        { id: "o", label: "Ask first", apps: [], policy: "onRequest" },
      ]),
    ).toBe("1 free, 2 counted, 1 on request");
  });

  /**
   * The one that would be a lie. An absent break-glass config means the safety
   * net is UP on the device (`BreakGlassCfg::safety_net`), so a summary reading
   * "off" for a guardian who has never opened this section would misdescribe
   * the ward's only way out of a lock the device can't reach the relay to lift.
   */
  it("reports emergency unlock as on when it has never been configured", () => {
    expect(lifelineSummary(buildLifeline(undefined))).toBe(
      "No numbers set · emergency unlock",
    );
    expect(
      lifelineSummary({
        numbers: [{ label: "Mum", number: "07700900000" }],
        emergencyServices: true,
      }),
    ).toBe("1 number · emergency services · emergency unlock");
  });

  it("reports emergency unlock as off only when it was explicitly turned off", () => {
    expect(
      lifelineSummary({
        numbers: [],
        breakGlass: { enabled: false, scope: "full", durationMinutes: 10 },
      }),
    ).toBe("No numbers set");
  });

  /** Half-filled rows are dropped before the wire, so they must not be counted. */
  it("does not count a half-typed number", () => {
    expect(
      lifelineSummary({
        numbers: [
          { label: "Mum", number: "07700900000" },
          { label: "", number: "" },
        ],
        breakGlass: { enabled: false, scope: "full", durationMinutes: 10 },
      }),
    ).toBe("1 number");
  });
});

/**
 * `buildApps` normalizes a saved `AppsPolicy` into the draft shape `apps`
 * section's dirty-check compares against. Dropping `askFirst` here — found
 * live-testing named times' on-request groups — made the Apps row's "changed"
 * dot stick forever the moment an on-request group existed: `effectiveApps`
 * (which DOES carry askFirst) could never equal this normalized "saved" shape,
 * even the instant after a successful save. The save itself was never wrong;
 * only the draft/saved comparison was blind to the one field that mattered.
 */
describe("buildApps", () => {
  it("carries askFirst through, so a saved on-request group reads as saved", () => {
    expect(
      buildApps({
        enabled: true,
        posture: "blocklist",
        blocked: ["com.instagram.android"],
        allowed: [],
        askFirst: ["com.instagram.android"],
      }),
    ).toEqual({
      enabled: true,
      posture: "blocklist",
      blocked: ["com.instagram.android"],
      allowed: [],
      askFirst: ["com.instagram.android"],
    });
  });

  it("omits askFirst entirely when empty, matching every other array here", () => {
    const out = buildApps({ enabled: true, posture: "blocklist", blocked: [], allowed: [] });
    expect(out.askFirst).toBeUndefined();
  });

  // "Remove from device" has exactly the askFirst failure mode above: it is a
  // dimension the draft/saved comparison reads, so dropping it here would
  // leave the Apps row showing unsaved changes for ever after the first
  // removal — the save itself being perfectly correct throughout.
  it("carries hidden through, so a saved removal reads as saved", () => {
    expect(
      buildApps({
        enabled: false,
        posture: "blocklist",
        blocked: [],
        allowed: [],
        hidden: ["com.samsung.android.game.gamehome"],
      }),
    ).toEqual({
      enabled: false,
      posture: "blocklist",
      blocked: [],
      allowed: [],
      hidden: ["com.samsung.android.game.gamehome"],
    });
  });

  it("omits hidden entirely when empty, matching every other array here", () => {
    const out = buildApps({
      enabled: true,
      posture: "blocklist",
      blocked: [],
      allowed: [],
      hidden: [],
    });
    expect(out.hidden).toBeUndefined();
  });
});

/**
 * The collapsed Apps row's count must match what the WIRE actually signs
 * (`appsToGrant` strips askFirst pkgs out of `allowed` under allowlist — see
 * `wire/clause.ts`'s N1 fix), not the raw stored `allowed` list — otherwise
 * the same pkg reads as "allowed" here and "on request" on the Named times
 * row at once (found in review, the presentation regression left by the N1
 * fix).
 */
describe("appsSummary", () => {
  const T0 = 1782734400;

  it("allowlist: excludes an askFirst pkg from the allowed count", () => {
    const summary = appsSummary(
      {
        enabled: true,
        posture: "allowlist",
        blocked: [],
        allowed: ["com.mojang.minecraftpe", "org.mozilla.fenix"],
        askFirst: ["com.mojang.minecraftpe"],
      },
      T0,
    );
    // Two pkgs are stored in `allowed`, but only ONE of them is actually
    // allowed on the wire — the other is on-request.
    expect(summary).toBe("Only 1 app allowed");
  });

  it("allowlist: an askFirst pkg not in allowed does not change the count", () => {
    const summary = appsSummary(
      {
        enabled: true,
        posture: "allowlist",
        blocked: [],
        allowed: ["org.mozilla.fenix"],
        askFirst: ["com.mojang.minecraftpe"],
      },
      T0,
    );
    expect(summary).toBe("Only 1 app allowed");
  });

  it("allowlist: no askFirst set at all counts allowed verbatim (unchanged case)", () => {
    const summary = appsSummary(
      { enabled: true, posture: "allowlist", blocked: [], allowed: ["a", "b", "c"] },
      T0,
    );
    expect(summary).toBe("Only 3 apps allowed");
  });

  it("blocklist: askFirst never touches the blocked count (unchanged)", () => {
    const summary = appsSummary(
      {
        enabled: true,
        posture: "blocklist",
        blocked: ["com.mojang.minecraftpe", "com.instagram.android"],
        allowed: [],
        askFirst: ["com.mojang.minecraftpe"],
      },
      T0,
    );
    expect(summary).toBe("2 apps blocked");
  });

  it("disabled reads as no app blocks regardless of posture or askFirst", () => {
    expect(
      appsSummary(
        { enabled: false, posture: "allowlist", blocked: [], allowed: ["a"], askFirst: ["a"] },
        T0,
      ),
    ).toBe("No app blocks");
  });

  // Removals survive `paused` on the wire (`appsToGrant` emits `hidden`
  // before the paused early-return), so the collapsed row has to say so: a
  // flat "No app blocks" for a tablet with three apps taken off it would hide
  // the only thing this section had actually done.
  it("names removals even with app control off", () => {
    expect(
      appsSummary(
        { enabled: false, posture: "blocklist", blocked: [], allowed: [], hidden: ["a", "b", "c"] },
        T0,
      ),
    ).toBe("No app blocks, 3 removed from device");
  });

  it("appends removals to the blocked count", () => {
    expect(
      appsSummary(
        { enabled: true, posture: "blocklist", blocked: ["a", "b"], allowed: [], hidden: ["c"] },
        T0,
      ),
    ).toBe("2 apps blocked, 1 removed");
  });

  it("appends removals to the allowed count too", () => {
    expect(
      appsSummary(
        { enabled: true, posture: "allowlist", blocked: [], allowed: ["a"], hidden: ["b", "c"] },
        T0,
      ),
    ).toBe("Only 1 app allowed, 2 removed");
  });

  it("says nothing extra when nothing is removed (unchanged case)", () => {
    expect(
      appsSummary({ enabled: true, posture: "blocklist", blocked: ["a"], allowed: [], hidden: [] }, T0),
    ).toBe("1 app blocked");
  });
});

describe("alwaysAvailableSummary", () => {
  const NOW = 1_700_000_000;

  it("says plainly when nothing is named", () => {
    expect(alwaysAvailableSummary({ apps: [] }, NOW)).toBe("Nothing — the lock takes everything");
  });

  it("names the apps when they are all standing", () => {
    expect(
      alwaysAvailableSummary({ apps: [{ pkg: "com.book" }, { pkg: "com.pods" }] }, NOW),
    ).toBe("2 apps, always");
  });

  it("counts a temporary grant separately, because it will lapse", () => {
    expect(
      alwaysAvailableSummary(
        { apps: [{ pkg: "com.book" }, { pkg: "com.chat", untilUnix: NOW + 3600 }] },
        NOW,
      ),
    ).toBe("1 app, always · 1 for now");
  });

  it("ignores an entry that has already lapsed", () => {
    expect(
      alwaysAvailableSummary({ apps: [{ pkg: "com.chat", untilUnix: NOW - 1 }] }, NOW),
    ).toBe("Nothing — the lock takes everything");
  });

  it("uses the singular for one app", () => {
    expect(alwaysAvailableSummary({ apps: [{ pkg: "com.book" }] }, NOW)).toBe("1 app, always");
  });
});

describe("buildAlwaysAvailable", () => {
  it("normalises an absent policy to an empty list", () => {
    expect(buildAlwaysAvailable(undefined)).toEqual({ apps: [] });
  });

  it("is idempotent, so a saved draft compares byte-identical", () => {
    const once = buildAlwaysAvailable({ apps: [{ pkg: "com.book" }] });
    expect(buildAlwaysAvailable(once)).toEqual(once);
  });
});
