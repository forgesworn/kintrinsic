import { describe, expect, it } from "vitest";
import {
  attachAllSignatures,
  humanLabelFor,
  identityDisplayLabel,
  isCmdlineIdentity,
  LAUNCH_SIGNATURES,
  shouldAttachSignature,
  signatureFor,
  stripAllSignatures,
  withSignature,
  withoutSignature,
} from "./launchSignatures";
import { groupsToClauses, type NamedGroup, type NamedTimesDraft, type PriorClauseState } from "./namedTimes";

const MINECRAFT_CMDLINE = "cmdline:net.minecraft.client.main.Main";

describe("LAUNCH_SIGNATURES", () => {
  it("has exactly one entry at launch — Minecraft Java", () => {
    expect(LAUNCH_SIGNATURES).toHaveLength(1);
    expect(LAUNCH_SIGNATURES[0].cmdlineId).toBe(MINECRAFT_CMDLINE);
  });
});

describe("signatureFor", () => {
  it("matches the flatpak id", () => {
    expect(signatureFor("com.mojang.Minecraft")?.cmdlineId).toBe(MINECRAFT_CMDLINE);
  });

  it("matches a full exec path by basename", () => {
    expect(signatureFor("/usr/bin/minecraft-launcher")?.cmdlineId).toBe(MINECRAFT_CMDLINE);
  });

  it("matches a bare basename with no path", () => {
    expect(signatureFor("minecraft-launcher")?.cmdlineId).toBe(MINECRAFT_CMDLINE);
  });

  it("matches nothing for an unrelated identity", () => {
    expect(signatureFor("org.mozilla.firefox")).toBeUndefined();
    expect(signatureFor("/usr/bin/prismlauncher")).toBeUndefined();
  });
});

describe("withSignature", () => {
  it("attaches the cmdline identity alongside a matched pkg", () => {
    expect(withSignature(["com.mojang.Minecraft"], "com.mojang.Minecraft")).toEqual([
      "com.mojang.Minecraft",
      MINECRAFT_CMDLINE,
    ]);
  });

  it("is idempotent — never duplicates on a repeat call", () => {
    const once = withSignature(["com.mojang.Minecraft"], "com.mojang.Minecraft");
    const twice = withSignature(once, "com.mojang.Minecraft");
    expect(twice).toEqual(once);
    expect(twice.filter((p) => p === MINECRAFT_CMDLINE)).toHaveLength(1);
  });

  it("is a no-op for a pkg with no signature", () => {
    expect(withSignature(["org.mozilla.firefox"], "org.mozilla.firefox")).toEqual(["org.mozilla.firefox"]);
  });
});

describe("withoutSignature", () => {
  it("removes the cmdline identity once its pkg is gone and nothing else needs it", () => {
    // The caller's own contract: remove `pkg` from the list first (a plain
    // filter), THEN call withoutSignature to clean up the orphaned identity.
    const apps = withSignature(["com.mojang.Minecraft"], "com.mojang.Minecraft");
    expect(apps).toContain(MINECRAFT_CMDLINE);
    const afterRemovingPkg = apps.filter((p) => p !== "com.mojang.Minecraft");
    expect(withoutSignature(afterRemovingPkg, "com.mojang.Minecraft")).toEqual([]);
  });

  it("keeps the cmdline identity when a DIFFERENT launcher of the same game remains", () => {
    // Both the flatpak id and the exec path match the same signature — the
    // group can legitimately carry both (belt and braces on the same ward).
    const apps = withSignature(
      withSignature(["com.mojang.Minecraft", "/usr/bin/minecraft-launcher"], "com.mojang.Minecraft"),
      "/usr/bin/minecraft-launcher",
    );
    const afterRemovingOne = withoutSignature(
      apps.filter((p) => p !== "com.mojang.Minecraft"),
      "com.mojang.Minecraft",
    );
    expect(afterRemovingOne).toContain(MINECRAFT_CMDLINE);
  });

  it("is a no-op for a pkg with no signature", () => {
    expect(withoutSignature(["org.mozilla.firefox"], "org.mozilla.firefox")).toEqual(["org.mozilla.firefox"]);
  });
});

describe("isCmdlineIdentity", () => {
  it("recognises the cmdline: prefix", () => {
    expect(isCmdlineIdentity(MINECRAFT_CMDLINE)).toBe(true);
    expect(isCmdlineIdentity("cmdline:/usr/lib")).toBe(true);
  });

  it("is false for an ordinary identity", () => {
    expect(isCmdlineIdentity("org.mozilla.firefox")).toBe(false);
    expect(isCmdlineIdentity("/usr/bin/minecraft-launcher")).toBe(false);
  });
});

// A raw `cmdline:` string must never reach a label a guardian or ward reads.
describe("humanLabelFor", () => {
  it("resolves a curated cmdline: identity to its signature's label", () => {
    expect(humanLabelFor(MINECRAFT_CMDLINE)).toBe("Minecraft");
  });

  it("never returns the raw needle for an UNRECOGNISED cmdline: string", () => {
    const label = humanLabelFor("cmdline:/usr/lib");
    expect(label).toBeDefined();
    expect(label).not.toMatch(/^cmdline:/);
  });

  it("returns undefined (defer to the caller's own fallback) for a non-cmdline identity", () => {
    expect(humanLabelFor("org.mozilla.firefox")).toBeUndefined();
  });
});

// The full picking flow (design §2.5): picking Minecraft into a group yields
// BOTH identities — the launcher pkg the guardian actually tapped, and the
// signature's cmdline: identity attached alongside it.
describe("picking Minecraft into a group", () => {
  it("yields both identities", () => {
    const group = withSignature([], "com.mojang.Minecraft");
    const withPkg = ["com.mojang.Minecraft", ...group];
    expect(withPkg).toEqual(["com.mojang.Minecraft", MINECRAFT_CMDLINE]);
  });
});

describe("identityDisplayLabel", () => {
  it("resolves a cmdline: label or pkg through humanLabelFor", () => {
    expect(identityDisplayLabel(undefined, MINECRAFT_CMDLINE)).toBe("Minecraft");
    expect(identityDisplayLabel(MINECRAFT_CMDLINE, undefined)).toBe("Minecraft");
  });

  it("prefers an explicit label over the pkg", () => {
    expect(identityDisplayLabel("VLC media player", "flatpak:org.videolan.VLC")).toBe("VLC media player");
  });

  it("falls back to the pkg when there's no label and it isn't cmdline:-shaped", () => {
    expect(identityDisplayLabel(undefined, "org.mozilla.firefox")).toBe("org.mozilla.firefox");
  });

  it("is undefined when both are absent", () => {
    expect(identityDisplayLabel(undefined, undefined)).toBeUndefined();
  });

  // New-4 (review, 2026-08-03): `wire/request.ts` accepts an empty-string
  // label on the wire. `label ?? pkg` would treat "" as present (only
  // null/undefined trigger `??`), swallowing the whole call to `undefined`
  // and pushing every caller back onto ITS OWN raw-pkg fallback — the one
  // surviving path a `cmdline:` needle could still print.
  it("falls through to pkg when label is an empty string, not just absent", () => {
    expect(identityDisplayLabel("", "org.mozilla.firefox")).toBe("org.mozilla.firefox");
    expect(identityDisplayLabel("", MINECRAFT_CMDLINE)).toBe("Minecraft");
  });
});

// F1 (review, 2026-08-03): the identity may attach ONLY into a Counted
// group — onRequest and free are actively harmful, not merely a no-op (see
// the function's own doc). Both gates (policy AND device capability) must
// hold.
describe("shouldAttachSignature", () => {
  it("is true only for counted + cmdlineOk", () => {
    expect(shouldAttachSignature("counted", true)).toBe(true);
  });

  it("is false for counted without device support", () => {
    expect(shouldAttachSignature("counted", false)).toBe(false);
  });

  it("is false for onRequest even when cmdlineOk", () => {
    expect(shouldAttachSignature("onRequest", true)).toBe(false);
  });

  it("is false for free even when cmdlineOk", () => {
    expect(shouldAttachSignature("free", true)).toBe(false);
  });
});

describe("stripAllSignatures", () => {
  it("removes every cmdline: identity, keeps everything else", () => {
    const apps = withSignature(["com.mojang.Minecraft", "org.mozilla.firefox"], "com.mojang.Minecraft");
    expect(stripAllSignatures(apps)).toEqual(["com.mojang.Minecraft", "org.mozilla.firefox"]);
  });

  it("is a no-op (same reference) when there's nothing to strip", () => {
    const apps = ["com.mojang.Minecraft"];
    expect(stripAllSignatures(apps)).toBe(apps);
  });
});

// New-3 (review, 2026-08-03): entering Counted re-attaches whatever the
// group's own apps already justify, so a Counted -> Free -> Counted
// round-trip doesn't quietly lose the identity with nothing to say so.
describe("attachAllSignatures", () => {
  it("attaches the identity for a matched pkg already in the list", () => {
    expect(attachAllSignatures(["com.mojang.Minecraft"])).toEqual([
      "com.mojang.Minecraft",
      MINECRAFT_CMDLINE,
    ]);
  });

  it("attaches once even when TWO launchers of the same game are both present", () => {
    const apps = attachAllSignatures(["com.mojang.Minecraft", "/usr/bin/minecraft-launcher"]);
    expect(apps.filter((p) => p === MINECRAFT_CMDLINE)).toHaveLength(1);
  });

  it("is idempotent on a list that already carries the identity", () => {
    const once = attachAllSignatures(["com.mojang.Minecraft"]);
    expect(attachAllSignatures(once)).toEqual(once);
  });

  it("is a no-op for a list with no signature match", () => {
    const apps = ["org.mozilla.firefox"];
    expect(attachAllSignatures(apps)).toBe(apps);
  });

  it("round-trips Counted -> Free -> Counted without losing the identity", () => {
    const counted = attachAllSignatures(["com.mojang.Minecraft"]);
    expect(counted).toContain(MINECRAFT_CMDLINE);
    const asFree = stripAllSignatures(counted); // leaving Counted
    expect(asFree).not.toContain(MINECRAFT_CMDLINE);
    const backToCounted = attachAllSignatures(asFree); // entering Counted again
    expect(backToCounted).toContain(MINECRAFT_CMDLINE);
  });
});

// End-to-end proof that F1's gate actually keeps a `cmdline:` identity out of
// the wire for onRequest/free, and that a counted group still gets it —
// mirroring exactly what NamedTimes.tsx's addApp does (gate via
// shouldAttachSignature, then compile via groupsToClauses).
describe("the compiled clause never carries a cmdline: identity outside Counted", () => {
  const MINECRAFT_PKG = "com.mojang.Minecraft";
  const prior: PriorClauseState = { buckets: { enabled: true, tz: "UTC", buckets: [] } };

  function attachIfEligible(apps: string[], pkg: string, groupPolicy: NamedGroup["policy"], cmdlineOk: boolean) {
    return shouldAttachSignature(groupPolicy, cmdlineOk) ? withSignature(apps, pkg) : apps;
  }

  function draftWith(groups: NamedGroup[]): NamedTimesDraft {
    return { groups, learningEnabled: true, bucketsEnabled: true };
  }

  it("onRequest: never attached, and the compiled askFirst/blocked stay clean", () => {
    const apps = attachIfEligible([MINECRAFT_PKG], MINECRAFT_PKG, "onRequest", true);
    expect(apps).toEqual([MINECRAFT_PKG]);
    const compiled = groupsToClauses(
      draftWith([{ id: "-on-request", label: "On request", apps, policy: "onRequest" }]),
      prior,
    );
    expect(compiled.apps.askFirst).not.toContain(MINECRAFT_CMDLINE);
    expect(compiled.apps.blockedAdd).not.toContain(MINECRAFT_CMDLINE);
  });

  it("free: never attached, and the compiled learning.apps stays clean", () => {
    const apps = attachIfEligible([MINECRAFT_PKG], MINECRAFT_PKG, "free", true);
    expect(apps).toEqual([MINECRAFT_PKG]);
    const compiled = groupsToClauses(
      draftWith([{ id: "-free", label: "Free time", apps, policy: "free" }]),
      prior,
    );
    expect(compiled.learning.apps.map((a) => a.id)).not.toContain(MINECRAFT_CMDLINE);
  });

  it("counted: still attaches, and the compiled bucket carries it", () => {
    const apps = attachIfEligible([MINECRAFT_PKG], MINECRAFT_PKG, "counted", true);
    expect(apps).toContain(MINECRAFT_CMDLINE);
    const compiled = groupsToClauses(
      draftWith([{ id: "play", label: "Play", apps, policy: "counted", dailyMinutes: 60 }]),
      prior,
    );
    expect(compiled.buckets.buckets[0].apps).toContain(MINECRAFT_CMDLINE);
  });

  it("switching a counted group carrying the identity away strips it before compile", () => {
    const counted = attachIfEligible([MINECRAFT_PKG], MINECRAFT_PKG, "counted", true);
    expect(counted).toContain(MINECRAFT_CMDLINE);
    const afterSwitch = stripAllSignatures(counted);
    expect(afterSwitch).toEqual([MINECRAFT_PKG]);
    const compiled = groupsToClauses(
      draftWith([{ id: "-on-request", label: "On request", apps: afterSwitch, policy: "onRequest" }]),
      prior,
    );
    expect(compiled.apps.askFirst).not.toContain(MINECRAFT_CMDLINE);
    expect(compiled.apps.blockedAdd).not.toContain(MINECRAFT_CMDLINE);
  });
});
