import { describe, expect, it } from "vitest";
import { appsToGrant, bucketsToGrant } from "../wire/clause";
import {
  applyAppsFragment,
  decompose,
  groupsToClauses,
  isDeviceShaped,
  siteIdentity,
  siteIdOf,
  learningAppPool,
  MAX_BUCKET_APPS,
  MAX_BUCKETS,
  moveApp,
  namedTimesError,
  namedTimeSlug,
  ON_REQUEST_LABEL,
  partitionOnPolicyChange,
  RESERVED_FREE_ID,
  RESERVED_ON_REQUEST_ID,
  stripManagedApps,
  type FreeGroupRecord,
  type NamedGroup,
  type NamedTimesDraft,
  type PriorClauseState,
} from "./namedTimes";
import type { AppsPolicy, BucketsPolicy, LearningPolicy } from "./types";

const emptyPrior: PriorClauseState = {};

function free(id: string, label: string, apps: string[] = []): NamedGroup {
  return { id, label, apps, policy: "free" };
}
function counted(
  id: string,
  label: string,
  apps: string[],
  extra: Partial<Pick<NamedGroup, "dailyMinutes" | "weeklyMinutes">> = { dailyMinutes: 60 },
): NamedGroup {
  return { id, label, apps, policy: "counted", ...extra };
}
function onRequest(id: string, label: string, apps: string[]): NamedGroup {
  return { id, label, apps, policy: "onRequest" };
}
function draft(groups: NamedGroup[], axis: Partial<Pick<NamedTimesDraft, "learningEnabled" | "bucketsEnabled">> = {}): NamedTimesDraft {
  return { groups, learningEnabled: true, bucketsEnabled: true, ...axis };
}
/** Pure JSON-shape dirty comparator, exactly what `Limits.tsx` uses to decide
 *  what a save signs — mirrored here so the C1/C2 regressions can be pinned
 *  without rendering the screen. */
function dirtyOf(a: ReturnType<typeof groupsToClauses>, b: ReturnType<typeof groupsToClauses>) {
  return {
    learning: JSON.stringify(a.learning) !== JSON.stringify(b.learning),
    buckets: JSON.stringify(a.buckets) !== JSON.stringify(b.buckets),
    freeGroups: JSON.stringify(a.freeGroups) !== JSON.stringify(b.freeGroups),
  };
}

describe("groupsToClauses", () => {
  it("compiles a free group into the learning fragment, membership persisted guardian-side only", () => {
    const groups = [free("f1", "Learning", ["khan-academy", "com.example.native"])];
    const out = groupsToClauses(draft(groups), emptyPrior);
    expect(out.learning.enabled).toBe(true);
    expect(out.learning.apps.map((a) => a.id).sort()).toEqual(["com.example.native", "khan-academy"]);
    // Catalogue entry resolves to its full shape (domains survive), not a bare native fallback.
    const khan = out.learning.apps.find((a) => a.id === "khan-academy");
    expect(khan?.kind).toBe("site");
    expect(khan?.domains?.length).toBeGreaterThan(0);
    // A bare pkg with no known detail resolves to a plain native entry.
    const native = out.learning.apps.find((a) => a.id === "com.example.native");
    expect(native).toEqual({ id: "com.example.native", label: "com.example.native", kind: "native", exec: "com.example.native" });
    // The name/membership lives ONLY in guardian-side bookkeeping.
    expect(out.freeGroups).toEqual([{ id: "f1", label: "Learning", apps: ["khan-academy", "com.example.native"] }]);
    expect(JSON.stringify(out.learning)).not.toContain("Learning");
  });

  it("passes capMinutes through as an advanced, per-policy (not per-group) knob", () => {
    const prior: PriorClauseState = { learning: { enabled: true, apps: [], capMinutes: 90 } };
    const out = groupsToClauses(draft([free("f1", "Learning", ["x"])]), prior);
    expect(out.learning.capMinutes).toBe(90);
  });

  it("learning.enabled is the axis flag verbatim, never derived from group presence (C2)", () => {
    const groups = [free("f1", "Learning", ["x"])];
    expect(groupsToClauses(draft(groups, { learningEnabled: true }), emptyPrior).learning.enabled).toBe(true);
    expect(groupsToClauses(draft(groups, { learningEnabled: false }), emptyPrior).learning.enabled).toBe(false);
    // Paused, but the app is STILL there — pausing must never delete configuration.
    expect(groupsToClauses(draft(groups, { learningEnabled: false }), emptyPrior).learning.apps).toHaveLength(1);
  });

  it("no free group means an empty app list, but enabled still follows the axis flag", () => {
    const out = groupsToClauses(draft([counted("c1", "Play", ["x"])], { learningEnabled: false }), emptyPrior);
    expect(out.learning.enabled).toBe(false);
    expect(out.learning.apps).toEqual([]);
  });

  it("compiles a counted group into a matching bucket, verbatim", () => {
    const groups = [counted("play", "Play", ["com.mojang.Minecraft"], { dailyMinutes: 60, weeklyMinutes: 300 })];
    const out = groupsToClauses(draft(groups), emptyPrior);
    expect(out.buckets.enabled).toBe(true);
    expect(out.buckets.buckets).toEqual([
      { id: "play", label: "Play", apps: ["com.mojang.Minecraft"], dailyMinutes: 60, weeklyMinutes: 300 },
    ]);
  });

  it("buckets.enabled is the axis flag verbatim, never derived from group presence (C2)", () => {
    const groups = [counted("play", "Play", ["x"])];
    const out = groupsToClauses(draft(groups, { bucketsEnabled: false }), emptyPrior);
    expect(out.buckets.enabled).toBe(false);
    // Paused, but the bucket is STILL there.
    expect(out.buckets.buckets).toHaveLength(1);
  });

  it("carries buckets' tz/weekStart through from prior state", () => {
    const prior: PriorClauseState = { buckets: { enabled: true, tz: "Europe/London", buckets: [], weekStart: "mon" } };
    const out = groupsToClauses(draft([counted("c1", "Play", ["x"])]), prior);
    expect(out.buckets.tz).toBe("Europe/London");
    expect(out.buckets.weekStart).toBe("mon");
  });

  it("falls back to UTC when no prior buckets tz is known", () => {
    const out = groupsToClauses(draft([counted("c1", "Play", ["x"])]), emptyPrior);
    expect(out.buckets.tz).toBe("UTC");
  });

  it("no counted groups means an empty bucket set, but enabled still follows the axis flag", () => {
    const out = groupsToClauses(draft([free("f1", "Learning", ["x"])], { bucketsEnabled: false }), emptyPrior);
    expect(out.buckets.enabled).toBe(false);
    expect(out.buckets.buckets).toEqual([]);
  });

  it("compiles an on-request group into blockedAdd AND askFirst — the subset invariant holds", () => {
    const groups = [onRequest("play", "Play", ["com.instagram.android", "com.tiktok"])];
    const out = groupsToClauses(draft(groups), emptyPrior);
    expect(out.apps.askFirst.sort()).toEqual(["com.instagram.android", "com.tiktok"]);
    expect(out.apps.askFirst.every((p) => out.apps.blockedAdd.includes(p))).toBe(true);
  });

  it("dedupes apps within an axis and trims/drops blanks", () => {
    const out = groupsToClauses(draft([onRequest("play", "Play", [" x ", "x", "", "  "])]), emptyPrior);
    expect(out.apps.askFirst).toEqual(["x"]);
    expect(out.apps.blockedAdd).toEqual(["x"]);
  });

  // Defence in depth (review, New-2): the picker gate (`shouldAttachSignature`
  // in `screens/NamedTimes.tsx`) only stops a NEW `cmdline:` identity from
  // being attached outside Counted — it does nothing for an already-saved
  // policy that carries one from before that gate existed. `groupsToClauses`
  // itself must never re-emit one for a non-counted group, regardless of how
  // it got there, so a hand-built "pre-fix" draft is the right test fixture
  // here rather than going through the picker at all.
  describe("a cmdline: identity in a non-counted group is stripped regardless of provenance (New-2)", () => {
    const MINECRAFT_CMDLINE = "cmdline:net.minecraft.client.main.Main";

    it("never reaches learning.apps from a free group", () => {
      const groups = [free("f1", "Play", ["com.mojang.Minecraft", MINECRAFT_CMDLINE])];
      const out = groupsToClauses(draft(groups), emptyPrior);
      expect(out.learning.apps.map((a) => a.id)).not.toContain(MINECRAFT_CMDLINE);
      expect(out.learning.apps.map((a) => a.id)).toContain("com.mojang.Minecraft");
    });

    it("never reaches askFirst/blockedAdd from an on-request group", () => {
      const groups = [onRequest("o1", "Play", ["com.mojang.Minecraft", MINECRAFT_CMDLINE])];
      const out = groupsToClauses(draft(groups), emptyPrior);
      expect(out.apps.askFirst).not.toContain(MINECRAFT_CMDLINE);
      expect(out.apps.blockedAdd).not.toContain(MINECRAFT_CMDLINE);
      expect(out.apps.askFirst).toContain("com.mojang.Minecraft");
    });

    it("a counted group is UNAFFECTED — the identity is legitimate there", () => {
      const groups = [counted("play", "Play", ["com.mojang.Minecraft", MINECRAFT_CMDLINE])];
      const out = groupsToClauses(draft(groups), emptyPrior);
      expect(out.buckets.buckets[0].apps).toContain(MINECRAFT_CMDLINE);
    });
  });

  it("caps counted groups at MAX_BUCKETS (defense in depth — validation is the honest gate)", () => {
    const groups = Array.from({ length: MAX_BUCKETS + 3 }, (_, i) => counted(`c${i}`, `C${i}`, [`app${i}`]));
    const out = groupsToClauses(draft(groups), emptyPrior);
    expect(out.buckets.buckets).toHaveLength(MAX_BUCKETS);
  });

  it("caps one group's apps at MAX_BUCKET_APPS (defense in depth)", () => {
    const apps = Array.from({ length: MAX_BUCKET_APPS + 10 }, (_, i) => `app${i}`);
    const out = groupsToClauses(draft([counted("c1", "Play", apps)]), emptyPrior);
    expect(out.buckets.buckets[0].apps).toHaveLength(MAX_BUCKET_APPS);
  });

  it("v-rule passthrough: a weekly-only counted group compiles to v:2 through bucketsToGrant", () => {
    const out = groupsToClauses(draft([counted("c1", "Play", ["x"], { weeklyMinutes: 300 })]), emptyPrior);
    const grant = bucketsToGrant(out.buckets, 1700);
    expect(grant.v).toBe(2);
    expect(grant.buckets[0].weeklyMinutes).toBe(300);
    expect(grant.buckets[0].dailyMinutes).toBeUndefined();
  });

  it("v-rule passthrough: an all-daily counted set compiles to v:1", () => {
    const out = groupsToClauses(draft([counted("c1", "Play", ["x"], { dailyMinutes: 60 })]), emptyPrior);
    const grant = bucketsToGrant(out.buckets, 1700);
    expect(grant.v).toBe(1);
  });

  it("a mixed save carries all three fragments independently", () => {
    const groups = [
      free("f1", "Learning", ["khan-academy"]),
      counted("c1", "Play", ["com.mojang.Minecraft"]),
      onRequest("o1", "Ask first", ["com.instagram.android"]),
    ];
    const out = groupsToClauses(draft(groups), emptyPrior);
    expect(out.learning.apps.map((a) => a.id)).toEqual(["khan-academy"]);
    expect(out.buckets.buckets.map((b) => b.id)).toEqual(["c1"]);
    expect(out.apps.askFirst).toEqual(["com.instagram.android"]);
  });

  // 2026-08-06 "named costs" design (§4.3): a `site:<id>` identity inside a
  // Counted or On-request group names a site app that COSTS rather than
  // being free. Before this, a site could only ever be free — one clause
  // both defined the pinned window and made it free.
  describe("a site: identity in a Counted/On-request group costs (§4.3)", () => {
    it("materialises a costing site into learning.apps, marked free: false, while the bucket names it by site:<id>", () => {
      const groups = [counted("play", "Play", [siteIdentity("khan-academy")], { dailyMinutes: 30 })];
      const out = groupsToClauses(draft(groups), emptyPrior);
      expect(out.learning.apps).toHaveLength(1);
      expect(out.learning.apps[0].id).toBe("khan-academy");
      expect(out.learning.apps[0].free).toBe(false);
      // The site's full catalogue shape resolves (domains survive) —
      // not a bare native fallback.
      expect(out.learning.apps[0].kind).toBe("site");
      expect(out.learning.apps[0].domains?.length).toBeGreaterThan(0);
      // The bucket itself still names the site by its site: identity —
      // that is what the device actually matches against.
      expect(out.buckets.buckets[0].apps).toEqual([siteIdentity("khan-academy")]);
    });

    it("materialises a costing site named by an on-request group the same way", () => {
      const groups = [onRequest("o1", "Ask first", [siteIdentity("khan-academy")])];
      const out = groupsToClauses(draft(groups), emptyPrior);
      expect(out.learning.apps.map((a) => a.id)).toEqual(["khan-academy"]);
      expect(out.learning.apps[0].free).toBe(false);
      expect(out.apps.askFirst).toEqual([siteIdentity("khan-academy")]);
    });

    it("a FREE group's site keeps `free` absent — never explicitly true (back-compat is the common case)", () => {
      const groups = [free("f1", "Learning", ["khan-academy"])];
      const out = groupsToClauses(draft(groups), emptyPrior);
      expect(out.learning.apps[0].free).toBeUndefined();
      expect("free" in out.learning.apps[0]).toBe(false);
    });

    it("a site truncated off a bucket by MAX_BUCKET_APPS leaves no phantom learning.apps entry", () => {
      const apps = Array.from({ length: MAX_BUCKET_APPS }, (_, i) => `app${i}`);
      apps.push(siteIdentity("khan-academy")); // pushed past the per-group cap
      const out = groupsToClauses(draft([counted("c1", "Play", apps, { dailyMinutes: 60 })]), emptyPrior);
      expect(out.buckets.buckets[0].apps).not.toContain(siteIdentity("khan-academy"));
      expect(out.learning.apps.some((a) => a.id === "khan-academy")).toBe(false);
    });

    it("defence in depth: the SAME site id both free and costing (violating the at-most-one-group invariant) never double-emits — free wins", () => {
      // Not reachable through the UI (`moveApp` enforces at-most-one-group),
      // but a hand-built/legacy draft must not desync `learning.apps` into
      // two entries for one id.
      const groups = [
        free("f1", "Learning", ["khan-academy"]),
        counted("play", "Play", [siteIdentity("khan-academy")], { dailyMinutes: 30 }),
      ];
      const out = groupsToClauses(draft(groups), emptyPrior);
      const khanEntries = out.learning.apps.filter((a) => a.id === "khan-academy");
      expect(khanEntries).toHaveLength(1);
      expect(khanEntries[0].free).toBeUndefined();
    });

    it("decompose round-trips a Counted group naming a costing site, without spawning a phantom free group", () => {
      const groups = [
        counted("play", "Play", [siteIdentity("khan-academy"), "com.mojang.Minecraft"], { dailyMinutes: 30 }),
      ];
      const compiled = groupsToClauses(draft(groups), emptyPrior);
      const back = decompose({ learning: compiled.learning, buckets: compiled.buckets }, compiled.freeGroups);
      expect(back.groups).toEqual(groups);
      expect(back.groups.some((g) => g.policy === "free")).toBe(false);
    });

    it("decompose round-trips an On-request group naming a costing site, without spawning a phantom free group", () => {
      const groups = [onRequest(RESERVED_ON_REQUEST_ID, ON_REQUEST_LABEL, [siteIdentity("khan-academy")])];
      const compiled = groupsToClauses(draft(groups), emptyPrior);
      const back = decompose(
        { learning: compiled.learning, apps: applyAppsFragment(undefined, compiled.apps) },
        compiled.freeGroups,
      );
      expect(back.groups).toEqual(groups);
      expect(back.groups.some((g) => g.policy === "free")).toBe(false);
    });

    it("a saved policy with a costing site round-trips through decompose∘compile with zero residual dirty", () => {
      const groups = [
        free("reading", "Reading", ["duolingo"]),
        counted("play", "Play", [siteIdentity("khan-academy")], { dailyMinutes: 30 }),
      ];
      const compiled = groupsToClauses(draft(groups), emptyPrior);
      const savedDraft = decompose(
        { learning: compiled.learning, buckets: compiled.buckets },
        compiled.freeGroups,
      );
      const recompiled = groupsToClauses(savedDraft, { learning: compiled.learning, buckets: compiled.buckets });
      expect(dirtyOf(compiled, recompiled)).toEqual({ learning: false, buckets: false, freeGroups: false });
    });
  });
});

describe("decompose", () => {
  it("round-trips a pre-named-times learning policy into a single synthetic free group", () => {
    // `savedFreeGroups` OMITTED (undefined) — a true pre-`freeGroups` family,
    // never once saved by this branch's code.
    const learning: LearningPolicy = {
      enabled: true,
      apps: [{ id: "khan-academy", label: "Khan Academy", kind: "site", url: "https://x", domains: ["x"] }],
    };
    const out = decompose({ learning });
    expect(out.groups).toEqual([{ id: RESERVED_FREE_ID, label: "Free time", apps: ["khan-academy"], policy: "free" }]);
    expect(out.learningEnabled).toBe(true);
  });

  it("reconstructs the free group's guardian-chosen membership when saved", () => {
    const learning: LearningPolicy = { enabled: true, apps: [{ id: "a", label: "A", kind: "native", exec: "a" }] };
    const saved: FreeGroupRecord[] = [{ id: "f1", label: "Homework", apps: ["a"] }];
    const out = decompose({ learning }, saved);
    expect(out.groups).toEqual([{ id: "f1", label: "Homework", apps: ["a"], policy: "free" }]);
  });

  it("C2: reads a paused learning axis into the model without losing its apps", () => {
    const learning: LearningPolicy = { enabled: false, apps: [{ id: "a", label: "A", kind: "native", exec: "a" }] };
    const out = decompose({ learning });
    expect(out.learningEnabled).toBe(false);
    expect(out.groups[0].apps).toEqual(["a"]);
  });

  it("an enabled-but-empty learning policy, never saved with freeGroups, still reconstructs a (empty) free group", () => {
    const out = decompose({ learning: { enabled: true, apps: [] } });
    expect(out.groups).toHaveLength(1);
    expect(out.groups[0].apps).toEqual([]);
  });

  it("a never-touched learning dimension means no free group at all", () => {
    const out = decompose({});
    expect(out.groups.some((g) => g.policy === "free")).toBe(false);
  });

  describe("undefined vs explicit [] for savedFreeGroups (regression: a phantom free group kept reappearing)", () => {
    it("an EXPLICIT empty freeGroups list with no wire apps means truly zero free groups", () => {
      // This is exactly what a guardian switching their only free group to
      // Counted/On-request produces on the next save: learning stays a
      // defined, enabled-but-empty object (nothing ever unsets it), but
      // freeGroups is saved as `[]` — that must mean zero, not "legacy,
      // please reconstruct one anyway". Before this fix, `groupsToClauses`
      // resurrected a phantom `{id:"-free", apps:[]}` container every time
      // this state was recompiled, which made `freeGroups` compare unequal
      // to itself forever (the save never settled).
      const out = decompose({ learning: { enabled: true, apps: [] } }, []);
      expect(out.groups.some((g) => g.policy === "free")).toBe(false);
    });

    it("an EXPLICIT empty freeGroups list still rescues a wire app no group claims (drift)", () => {
      const learning: LearningPolicy = { enabled: true, apps: [{ id: "a", label: "A", kind: "native", exec: "a" }] };
      const out = decompose({ learning }, []);
      expect(out.groups).toEqual([{ id: RESERVED_FREE_ID, label: "Free time", apps: ["a"], policy: "free" }]);
    });

    it("decompose∘compile settles after explicitly emptying free groups — no residual dirty", () => {
      const compiledAfterClearing = groupsToClauses(draft([]), { learning: { enabled: true, apps: [] } });
      expect(compiledAfterClearing.freeGroups).toEqual([]);
      const savedDraft = decompose({ learning: compiledAfterClearing.learning }, compiledAfterClearing.freeGroups);
      const savedCompiled = groupsToClauses(savedDraft, { learning: compiledAfterClearing.learning });
      expect(dirtyOf(compiledAfterClearing, savedCompiled)).toEqual({ learning: false, buckets: false, freeGroups: false });
    });
  });

  it("reconstructs one counted group per bucket, verbatim", () => {
    const buckets: BucketsPolicy = {
      enabled: true,
      tz: "UTC",
      buckets: [
        { id: "play", label: "Play", apps: ["mc"], dailyMinutes: 60 },
        { id: "chat", label: "Chat", apps: ["discord"], weeklyMinutes: 300 },
      ],
    };
    const out = decompose({ buckets }, []);
    expect(out.groups).toEqual([
      { id: "play", label: "Play", apps: ["mc"], policy: "counted", dailyMinutes: 60 },
      { id: "chat", label: "Chat", apps: ["discord"], policy: "counted", weeklyMinutes: 300 },
    ]);
  });

  it("C2: reads a paused buckets axis into the model without losing its groups", () => {
    const buckets: BucketsPolicy = { enabled: false, tz: "UTC", buckets: [{ id: "play", label: "Play", apps: ["mc"], dailyMinutes: 60 }] };
    const out = decompose({ buckets }, []);
    expect(out.bucketsEnabled).toBe(false);
    expect(out.groups).toHaveLength(1);
  });

  it("reconstructs a single on-request group from askFirst, with a fixed generic label", () => {
    const apps: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["ig", "manual"], allowed: [], askFirst: ["ig"] };
    const out = decompose({ apps }, []);
    expect(out.groups).toEqual([{ id: RESERVED_ON_REQUEST_ID, label: ON_REQUEST_LABEL, apps: ["ig"], policy: "onRequest" }]);
  });

  it("no askFirst means no on-request group", () => {
    const apps: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual"], allowed: [] };
    const out = decompose({ apps }, []);
    expect(out.groups.some((g) => g.policy === "onRequest")).toBe(false);
  });

  it("decompose(compile(...)) round-trips a mixed group set", () => {
    const groups: NamedGroup[] = [
      free("free-1", "Reading", ["khan-academy"]),
      counted("play", "Play", ["com.mojang.Minecraft"], { dailyMinutes: 60, weeklyMinutes: 300 }),
      onRequest(RESERVED_ON_REQUEST_ID, ON_REQUEST_LABEL, ["com.instagram.android"]),
    ];
    const compiled = groupsToClauses(draft(groups), emptyPrior);
    const back = decompose(
      { learning: compiled.learning, buckets: compiled.buckets, apps: applyAppsFragment(undefined, compiled.apps) },
      compiled.freeGroups,
    );
    expect(back.groups).toEqual(groups);
    expect(back.learningEnabled).toBe(true);
    expect(back.bucketsEnabled).toBe(true);
  });

  describe("I6: multiple free groups persist and round-trip losslessly", () => {
    it("keeps two free groups distinct across a save/reload cycle", () => {
      const groups = [free("reading", "Reading", ["khan-academy"]), free("games", "Educational games", ["duolingo"])];
      const compiled = groupsToClauses(draft(groups), emptyPrior);
      expect(compiled.freeGroups).toEqual([
        { id: "reading", label: "Reading", apps: ["khan-academy"] },
        { id: "games", label: "Educational games", apps: ["duolingo"] },
      ]);
      const back = decompose({ learning: compiled.learning }, compiled.freeGroups);
      expect(back.groups).toEqual(groups);
    });

    it("a compile→decompose round trip settles (no residual dirty)", () => {
      const groups = [free("reading", "Reading", ["khan-academy"]), free("games", "Games", ["duolingo"])];
      const d = draft(groups);
      const compiled = groupsToClauses(d, emptyPrior);
      const redecomposed = decompose({ learning: compiled.learning }, compiled.freeGroups);
      const recompiled = groupsToClauses(redecomposed, { learning: compiled.learning });
      expect(dirtyOf(compiled, recompiled)).toEqual({ learning: false, buckets: false, freeGroups: false });
    });

    it("reconciles a wire app claimed by no saved group into the first group (drift)", () => {
      const learning: LearningPolicy = {
        enabled: true,
        apps: [
          { id: "khan-academy", label: "Khan Academy", kind: "site", url: "x", domains: ["x"] },
          { id: "duolingo", label: "Duolingo", kind: "site", url: "x", domains: ["x"] },
          { id: "stray", label: "Stray", kind: "native", exec: "stray" },
        ],
      };
      const saved: FreeGroupRecord[] = [
        { id: "reading", label: "Reading", apps: ["khan-academy"] },
        { id: "games", label: "Games", apps: ["duolingo"] },
      ];
      const out = decompose({ learning }, saved);
      expect(out.groups.find((g) => g.id === "reading")?.apps).toEqual(["khan-academy", "stray"]);
      expect(out.groups.find((g) => g.id === "games")?.apps).toEqual(["duolingo"]);
    });

    it("drops a saved group's app the wire no longer carries", () => {
      const learning: LearningPolicy = { enabled: true, apps: [] };
      const saved: FreeGroupRecord[] = [{ id: "reading", label: "Reading", apps: ["khan-academy"] }];
      const out = decompose({ learning }, saved);
      expect(out.groups).toEqual([{ id: "reading", label: "Reading", apps: [], policy: "free" }]);
    });
  });

  describe("N1: a freeGroups-only save (no learning clause yet) round-trips (review round 2)", () => {
    /**
     * A family with no learning clause at all creates an empty Free group and
     * saves before adding any app: `compiled.learning` (`{enabled:true,
     * apps:[]}`, from the axis default) equals what a fresh decompose of
     * `clauses.learning === undefined` also compiles to, so `learningDirty` is
     * false and the save NEVER touches `learning` — only `freeGroups` goes
     * out. Reconstructing that saved group used to be gated on `learning`
     * being defined, which it never becomes, so the group was silently
     * dropped on the very next recompute (and a cold reload).
     */
    it("reconstructs a free group whose save carried freeGroups but never touched learning", () => {
      const freeGroups: FreeGroupRecord[] = [{ id: "play-time", label: "Play time", apps: [] }];
      const out = decompose({}, freeGroups);
      expect(out.groups).toEqual([{ id: "play-time", label: "Play time", apps: [], policy: "free" }]);
    });

    it("settles clean immediately after such a save — no residual dirty", () => {
      const compiledFirstSave = groupsToClauses(draft([free("play-time", "Play time", [])]), {});
      expect(compiledFirstSave.freeGroups).toEqual([{ id: "play-time", label: "Play time", apps: [] }]);
      // learning was never dirtied by this save (no apps chosen), so the
      // "prior" a real mount would recompile against still has no learning
      // clause — exactly what made this regress.
      const savedDraft = decompose({}, compiledFirstSave.freeGroups);
      const savedCompiled = groupsToClauses(savedDraft, {});
      expect(dirtyOf(compiledFirstSave, savedCompiled)).toEqual({ learning: false, buckets: false, freeGroups: false });
    });

    it("survives a simulated cold reload (learning still undefined, freeGroups the only saved trace)", () => {
      const freeGroups: FreeGroupRecord[] = [{ id: "play-time", label: "Play time", apps: [] }];
      const reloaded = decompose({ learning: undefined, buckets: undefined, apps: undefined }, freeGroups);
      expect(reloaded.groups.map((g) => g.id)).toContain("play-time");
    });
  });
});

describe("moveApp", () => {
  const base: NamedGroup[] = [
    { id: "a", label: "A", apps: ["x", "y"], policy: "free" },
    { id: "b", label: "B", apps: ["z"], policy: "counted", dailyMinutes: 30 },
  ];

  it("adds a fresh app with no prior group and no move to report", () => {
    const { groups, movedFrom } = moveApp(base, "new-app", "b");
    expect(movedFrom).toBeUndefined();
    expect(groups.find((g) => g.id === "b")?.apps).toEqual(["z", "new-app"]);
    expect(groups.find((g) => g.id === "a")?.apps).toEqual(["x", "y"]);
  });

  it("moves an app already in a different group — never duplicates it", () => {
    const { groups, movedFrom } = moveApp(base, "x", "b");
    expect(movedFrom).toBe("a");
    expect(groups.find((g) => g.id === "a")?.apps).toEqual(["y"]);
    expect(groups.find((g) => g.id === "b")?.apps).toEqual(["z", "x"]);
    const owners = groups.filter((g) => g.apps.includes("x"));
    expect(owners).toHaveLength(1);
  });

  it("moving an app to the group it's already in is a no-op", () => {
    const { groups, movedFrom } = moveApp(base, "x", "a");
    expect(movedFrom).toBeUndefined();
    expect(groups).toEqual(base);
  });

  it("throws for an unknown target group", () => {
    expect(() => moveApp(base, "x", "does-not-exist")).toThrow();
  });
});

describe("namedTimesError", () => {
  it("refuses a counted name the wire would silently drop (over 32 characters)", () => {
    const long = "Play time on the tablet in the evening"; // 38
    expect(namedTimesError([counted("c", long, ["x"], { dailyMinutes: 60 })])).toMatch(/too long a name/);
    expect(namedTimesError([counted("c", "x".repeat(32), ["x"], { dailyMinutes: 60 })])).toBeNull();
    // Free and on-request names never ride the buckets clause.
    expect(namedTimesError([free("f", long, [])])).toBeNull();
  });

  it("is null for a clean, valid set", () => {
    expect(namedTimesError([free("f", "Learning", []), counted("c", "Play", ["x"], { dailyMinutes: 60 })])).toBeNull();
  });

  it("requires a name on every group", () => {
    expect(namedTimesError([free("f", "  ", [])])).toMatch(/name/i);
  });

  it("requires an axis (daily or weekly) on every counted group", () => {
    const noAxis: NamedGroup = { id: "c", label: "Play", apps: ["x"], policy: "counted" };
    expect(namedTimesError([noAxis])).toMatch(/daily or weekly/i);
  });

  it("free and on-request groups never need an axis", () => {
    expect(namedTimesError([free("f", "Learning", []), onRequest("o", "Ask", ["x"])])).toBeNull();
  });

  it("caps counted groups at MAX_BUCKETS", () => {
    const groups = Array.from({ length: MAX_BUCKETS + 1 }, (_, i) => counted(`c${i}`, `C${i}`, ["x"]));
    expect(namedTimesError(groups)).toMatch(new RegExp(String(MAX_BUCKETS)));
  });

  it("caps one group's apps at MAX_BUCKET_APPS", () => {
    const apps = Array.from({ length: MAX_BUCKET_APPS + 1 }, (_, i) => `app${i}`);
    expect(namedTimesError([counted("c", "Play", apps, { dailyMinutes: 60 })])).toMatch(/apps/i);
  });
});

describe("applyAppsFragment", () => {
  // "Remove from device" belongs to the classic Apps section, not to named
  // times — and this function rebuilds the policy from scratch, so a
  // dimension it forgets to copy is silently deleted on the very next save
  // (exactly the `allowed` bug the N1 tests below guard against).
  it("carries hidden through untouched", () => {
    const prior: AppsPolicy = {
      enabled: true,
      posture: "blocklist",
      blocked: ["yt"],
      allowed: [],
      hidden: ["com.samsung.android.game.gamehome"],
    };
    const out = applyAppsFragment(prior, { blockedAdd: ["ig"], askFirst: ["ig"] });
    expect(out.hidden).toEqual(["com.samsung.android.game.gamehome"]);
  });

  it("omits hidden when the prior policy had none (byte-identical to before)", () => {
    const out = applyAppsFragment(undefined, { blockedAdd: [], askFirst: [] });
    expect(out).not.toHaveProperty("hidden");
  });

  it("layers on-request pkgs onto an empty prior apps policy", () => {
    const out = applyAppsFragment(undefined, { blockedAdd: ["ig"], askFirst: ["ig"] });
    expect(out).toEqual({ enabled: true, posture: "blocklist", blocked: ["ig"], allowed: [], askFirst: ["ig"] });
  });

  it("preserves a manual block untouched by named times", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual-app"], allowed: [] };
    const out = applyAppsFragment(prior, { blockedAdd: [], askFirst: [] });
    expect(out.blocked).toEqual(["manual-app"]);
    expect(out.askFirst).toBeUndefined();
  });

  it("unions a manual block with a new on-request pkg", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual-app"], allowed: [] };
    const out = applyAppsFragment(prior, { blockedAdd: ["ig"], askFirst: ["ig"] });
    expect(out.blocked.sort()).toEqual(["ig", "manual-app"]);
    expect(out.askFirst).toEqual(["ig"]);
  });

  it("lifts a stale on-request pkg once its group is gone, keeping manual blocks", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual-app", "ig"], allowed: [], askFirst: ["ig"] };
    const out = applyAppsFragment(prior, { blockedAdd: [], askFirst: [] });
    expect(out.blocked).toEqual(["manual-app"]);
    expect(out.askFirst).toBeUndefined();
  });

  it("never turns enabled back off — a monotonic ratchet, so it can't undo a manual setting", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: [], allowed: [] };
    const out = applyAppsFragment(prior, { blockedAdd: [], askFirst: [] });
    expect(out.enabled).toBe(true);
  });

  it("preserves holds verbatim", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: [], allowed: [], holds: [{ pkg: "x", state: "allowed", untilUnix: 1700 }] };
    const out = applyAppsFragment(prior, { blockedAdd: [], askFirst: [] });
    expect(out.holds).toEqual([{ pkg: "x", state: "allowed", untilUnix: 1700 }]);
  });

  /**
   * N3 regression (review round 2): `putHold` legitimately produces an
   * explicit `holds: []` (every hold just lifted — see `domain/appHolds.ts`).
   * Dropping the key on an empty array made this function non-identity on
   * such a value: a split device's override carrying `holds: []` compared
   * unequal to itself every render (`effectiveOverrides` in Limits.tsx),
   * reading permanently dirty and re-signing an untouched override.
   */
  it("is identity-preserving on an explicit empty holds array — not just a non-empty one", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual"], allowed: [], holds: [] };
    const out = applyAppsFragment(prior, { blockedAdd: [], askFirst: [] });
    expect(out).toEqual(prior);
    expect(out.holds).toEqual([]);
  });

  it("still omits holds entirely when prior never had the key at all", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: [], allowed: [] };
    const out = applyAppsFragment(prior, { blockedAdd: [], askFirst: [] });
    expect(out.holds).toBeUndefined();
  });

  it("is idempotent — reapplying the same fragment to its own output changes nothing (I4 composition safety)", () => {
    const prior: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual"], allowed: [] };
    const fragment = { blockedAdd: ["ig"], askFirst: ["ig"] };
    const once = applyAppsFragment(prior, fragment);
    const twice = applyAppsFragment(once, fragment);
    expect(twice).toEqual(once);
  });

  // N1 (review round 2): `applyAppsFragment` must NEVER strip `allowed` — the
  // strip belongs at the wire (`appsToGrant`) only. `prior` here is not
  // always the pre-save draft: `Limits.tsx` signs this function's own output,
  // and the next render re-seeds `draftApps`/`savedApps` FROM that saved
  // policy, so `prior` routinely IS the persisted truth. A strip here would
  // therefore delete the guardian's stored allowlist entry permanently —
  // deleting the on-request group later would have nothing left to restore
  // Minecraft FROM (proved below by actually crossing a save boundary, not
  // just calling this pure function twice on the same never-saved prior).
  describe("N1: applyAppsFragment carries allowed through untouched, even under allowlist with askFirst", () => {
    it("does NOT strip a fragment askFirst pkg out of an existing allowed list", () => {
      const prior: AppsPolicy = {
        enabled: true,
        posture: "allowlist",
        blocked: [],
        allowed: ["com.mojang.minecraftpe", "org.mozilla.fenix"],
      };
      const out = applyAppsFragment(prior, { blockedAdd: ["com.mojang.minecraftpe"], askFirst: ["com.mojang.minecraftpe"] });
      expect(out.allowed.sort()).toEqual(["com.mojang.minecraftpe", "org.mozilla.fenix"]);
      expect(out.askFirst).toEqual(["com.mojang.minecraftpe"]);
    });

    /**
     * The save-boundary regression the reviewer's probe caught: the SAME
     * pure-function round-trip that looked fine calling `applyAppsFragment`
     * twice on one never-saved `prior` fails once `prior` is what a save
     * actually persists (this function's own output, re-fed in as next
     * render's saved policy — exactly what `Limits.tsx` does). Composed
     * result → treat as the new stored policy → delete the on-request group
     * → recompile against that stored policy → Minecraft must still be in
     * `allowed`, because nothing ever deleted it from the STORED policy —
     * only the wire bytes dropped it.
     */
    it("SAVE-BOUNDARY: deleting the on-request group after a save restores the allowlist entry (N1 regression)", () => {
      const savedBeforeGroup: AppsPolicy = {
        enabled: true,
        posture: "allowlist",
        blocked: [],
        allowed: ["com.mojang.minecraftpe", "org.mozilla.fenix"],
      };
      const withGroupFragment = { blockedAdd: ["com.mojang.minecraftpe"], askFirst: ["com.mojang.minecraftpe"] };

      // Save #1: the guardian adds the on-request group. `Limits.tsx` signs
      // this composed result — simulate that by treating IT as the next
      // render's saved/stored policy (the save boundary).
      const composedAtSave1 = applyAppsFragment(savedBeforeGroup, withGroupFragment);
      expect(composedAtSave1.allowed).toContain("com.mojang.minecraftpe");
      const storedAfterSave1 = composedAtSave1;

      // Later: the guardian deletes the on-request group — the fragment is
      // now empty — and recompiles against the ACTUAL stored policy from
      // save #1 (not the original pre-group prior).
      const composedAtSave2 = applyAppsFragment(storedAfterSave1, { blockedAdd: [], askFirst: [] });

      // Minecraft must still be in `allowed`: nothing ever removed it from
      // the STORED policy, so there is nothing to "restore" — it was simply
      // never gone. Before the N1 fix, save #1 would have stripped it from
      // `storedAfterSave1.allowed` for real, and save #2 would have had no
      // copy of it left anywhere to bring back.
      expect(composedAtSave2.allowed).toContain("com.mojang.minecraftpe");
      expect(composedAtSave2.askFirst).toBeUndefined();
    });
  });

  // F1c: end-to-end compile coverage — an on-request app that ALSO sits in
  // the classic Apps section's allowlist is advertised as askable AND
  // stripped from `allowed` on the signed WIRE clause (proving enforcement,
  // not just editor copy, is honest under allowlist posture) — while the
  // composed `AppsPolicy` a save actually persists keeps `allowed` intact
  // (N1: the strip is wire-only, never destructive to the stored policy).
  it("F1c: an on-request app also in allowed is stripped + askFirst ONLY on the wire, never in the stored policy", () => {
    const g = draft([onRequest("on-request", ON_REQUEST_LABEL, ["com.mojang.minecraftpe"])]);
    const compiled = groupsToClauses(g, emptyPrior);
    const priorApps: AppsPolicy = {
      enabled: true,
      posture: "allowlist",
      blocked: [],
      allowed: ["com.mojang.minecraftpe", "org.mozilla.fenix"],
    };
    const effectiveApps = applyAppsFragment(priorApps, compiled.apps);
    // The STORED policy (what a save actually persists) keeps Minecraft in
    // `allowed` — untouched by named times.
    expect(effectiveApps.allowed.sort()).toEqual(["com.mojang.minecraftpe", "org.mozilla.fenix"]);
    expect(effectiveApps.askFirst).toEqual(["com.mojang.minecraftpe"]);

    const wire = appsToGrant(effectiveApps, 1700);
    // The SIGNED bytes no longer allow it everywhere…
    expect(wire.allowed).toEqual(["org.mozilla.fenix"]);
    // …and DO carry the ask affordance the ward's surface renders.
    expect(wire.askFirst).toEqual(["com.mojang.minecraftpe"]);
  });
});

describe("stripManagedApps (I3)", () => {
  it("removes named-times-managed pkgs from the editable blocked/askFirst lists", () => {
    const apps: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual", "ig"], allowed: [], askFirst: ["ig"] };
    const out = stripManagedApps(apps, new Set(["ig"]));
    expect(out.blocked).toEqual(["manual"]);
    expect(out.askFirst).toBeUndefined();
  });

  it("leaves an unmanaged policy untouched (identity) when nothing is managed", () => {
    const apps: AppsPolicy = { enabled: true, posture: "blocklist", blocked: ["manual"], allowed: [] };
    expect(stripManagedApps(apps, new Set())).toBe(apps);
  });

  it("also strips a managed pkg from an allowlist posture's allowed list", () => {
    const apps: AppsPolicy = { enabled: true, posture: "allowlist", blocked: [], allowed: ["manual", "ig"] };
    const out = stripManagedApps(apps, new Set(["ig"]));
    expect(out.allowed).toEqual(["manual"]);
  });
});

describe("learningAppPool", () => {
  it("prefers the catalogue entry over a saved shape for domain-pin freshness", () => {
    // A stale pin: the SAME site, saved with an older domain list.
    const stale = { id: "khan-academy", label: "Khan Academy (old)", kind: "site" as const, url: "https://khanacademy.org/", domains: ["old-domain.example"] };
    const pool = learningAppPool([stale]);
    expect(pool.get("khan-academy")?.domains).not.toEqual(["old-domain.example"]);
    // …and an entry with no URL of its own can only be the catalogue's.
    const bare = { id: "khan-academy", label: "Khan", kind: "site" as const };
    expect(learningAppPool([bare]).get("khan-academy")?.url).toBe("https://www.khanacademy.org/");
  });

  it("never swaps the catalogue's entry in over a guardian's own site that shares its id", () => {
    // Saved before new sites were barred from minting a catalogue id.
    const mine = { id: "wikipedia", label: "Wikipedia", kind: "site" as const, url: "https://simple.wikipedia.org/wiki/Main_Page", domains: ["simple.wikipedia.org"] };
    expect(learningAppPool([mine]).get("wikipedia")).toEqual(mine);
  });

  it("dedupes a legacy label-slug native entry by its real pkg too (I2 minor)", () => {
    const legacy = { id: "minecraft", label: "Minecraft", kind: "native" as const, exec: "com.mojang.Minecraft" };
    const pool = learningAppPool([legacy]);
    expect(pool.get("com.mojang.Minecraft")).toEqual(legacy);
    expect(pool.get("minecraft")).toEqual(legacy);
  });
});

describe("namedTimeSlug", () => {
  it("slugifies a label", () => {
    expect(namedTimeSlug("Play Time!", [])).toBe("play-time");
  });

  it("disambiguates a taken slug", () => {
    expect(namedTimeSlug("Play", ["play"])).toBe("play-2");
  });

  it("never produces an empty slug", () => {
    expect(namedTimeSlug("???", [])).toBe("named-time");
  });

  it("never produces a leading hyphen — the reserved-id namespace stays collision-free", () => {
    for (const label of ["Free", "On request", "---", "-x", "x-"]) {
      expect(namedTimeSlug(label, []).startsWith("-")).toBe(false);
    }
  });
});

describe("reserved ids never collide with a guardian-typed slug (minor: reserved-id collision)", () => {
  it("a bucket literally named 'Free' does not collide with RESERVED_FREE_ID", () => {
    expect(namedTimeSlug("Free", [])).not.toBe(RESERVED_FREE_ID);
  });

  it("a bucket literally named 'On request' does not collide with RESERVED_ON_REQUEST_ID", () => {
    expect(namedTimeSlug("On request", [])).not.toBe(RESERVED_ON_REQUEST_ID);
  });
});

describe("isDeviceShaped / partitionOnPolicyChange (I5)", () => {
  it("recognises an Android package id and a Linux exec path as device-shaped", () => {
    expect(isDeviceShaped("com.mojang.Minecraft")).toBe(true);
    expect(isDeviceShaped("/usr/games/supertux2")).toBe(true);
  });

  it("rejects a catalogue or custom-site slug", () => {
    expect(isDeviceShaped("khan-academy")).toBe(false);
    expect(isDeviceShaped("maths-genie")).toBe(false);
  });

  it("partitions a mixed list, keeping device ids and naming the dropped ones", () => {
    const { kept, dropped } = partitionOnPolicyChange(["com.mojang.Minecraft", "khan-academy", "/usr/bin/x"]);
    expect(kept.sort()).toEqual(["/usr/bin/x", "com.mojang.Minecraft"]);
    expect(dropped).toEqual(["khan-academy"]);
  });

  it("keeps everything when the whole list is device-shaped", () => {
    expect(partitionOnPolicyChange(["com.a", "com.b"]).dropped).toEqual([]);
  });

  // The `site:` form is what lets a WEBSITE carry a policy. Before it, a site
  // could only ever be free — one clause both defined the pinned window and
  // made it free — so switching a group away from free silently dropped every
  // site in it, and "YouTube is half an hour a day" could not be expressed.
  it("recognises a site: identity as device-shaped, but not a bare catalogue slug", () => {
    expect(isDeviceShaped(siteIdentity("youtube"))).toBe(true);
    expect(isDeviceShaped("site:khan-academy")).toBe(true);
    // The bare slug names an entry in the guardian's own list, not a thing
    // running on a machine — still correctly rejected.
    expect(isDeviceShaped("khan-academy")).toBe(false);
  });

  it("survives a policy change instead of being dropped, which is the whole point", () => {
    const { kept, dropped } = partitionOnPolicyChange([
      siteIdentity("youtube"),
      "khan-academy",
      "com.mojang.Minecraft",
    ]);
    expect(kept.sort()).toEqual(["com.mojang.Minecraft", "site:youtube"]);
    expect(dropped).toEqual(["khan-academy"]);
  });

  it("round-trips an id through siteIdentity / siteIdOf", () => {
    expect(siteIdOf(siteIdentity("bbc-bitesize"))).toBe("bbc-bitesize");
    expect(siteIdOf("site:  youtube  ")).toBe("youtube");
  });

  it("treats a malformed site: identity as not device-shaped", () => {
    expect(siteIdOf("site:")).toBeNull();
    expect(siteIdOf("site:   ")).toBeNull();
    expect(isDeviceShaped("site:")).toBe(false);
    expect(siteIdOf("/usr/bin/firefox")).toBeNull();
  });

  // Mirrors `charter_schedule::clause`'s ordering test on the Rust side: a
  // site id may legitimately contain a dot, which is also what makes an
  // identity look like a flatpak/Android id to the enforcer. Both sides must
  // test the `site:` form FIRST.
  it("accepts a site id containing a dot", () => {
    expect(isDeviceShaped("site:my.school")).toBe(true);
    expect(siteIdOf("site:my.school")).toBe("my.school");
  });
});

describe("C1: mount-clean / no-spurious-clause regression (mirrors Limits.tsx's dirty wiring)", () => {
  it("a pre-named-times learning-only policy round-trips to zero dirty on mount", () => {
    const clauses: PriorClauseState = {
      learning: { enabled: true, apps: [{ id: "khan-academy", label: "Khan Academy", kind: "site", url: "x", domains: ["x"] }] },
    };
    const savedDraft = decompose(clauses);
    const savedCompiled = groupsToClauses(savedDraft, clauses);
    // The mounted draft is built the identical way — same decompose call.
    const mountedDraft = decompose(clauses);
    const mountedCompiled = groupsToClauses(mountedDraft, clauses);
    expect(dirtyOf(mountedCompiled, savedCompiled)).toEqual({ learning: false, buckets: false, freeGroups: false });
  });

  it("adding a counted group to a learning-only family dirties buckets only — learning is never re-signed", () => {
    const clauses: PriorClauseState = { learning: { enabled: true, apps: [] } };
    const savedDraft = decompose(clauses);
    const savedCompiled = groupsToClauses(savedDraft, clauses);
    const editedDraft: NamedTimesDraft = {
      ...savedDraft,
      groups: [...savedDraft.groups, counted("play", "Play", ["x"])],
    };
    const compiled = groupsToClauses(editedDraft, clauses);
    const dirty = dirtyOf(compiled, savedCompiled);
    expect(dirty.buckets).toBe(true);
    expect(dirty.learning).toBe(false);
  });

  it("a family with neither learning nor buckets ever touched mounts with zero groups and zero dirty", () => {
    const clauses: PriorClauseState = {};
    const savedDraft = decompose(clauses);
    expect(savedDraft.groups).toEqual([]);
    const savedCompiled = groupsToClauses(savedDraft, clauses);
    const mountedCompiled = groupsToClauses(decompose(clauses), clauses);
    expect(dirtyOf(mountedCompiled, savedCompiled)).toEqual({ learning: false, buckets: false, freeGroups: false });
  });
});
