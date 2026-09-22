import { describe, it, expect } from "vitest";
import { generateSecretKey, getPublicKey, finalizeEvent, nip44 } from "nostr-tools";
import {
  parseStatus,
  unwrapStatus,
  CHARTER_DEVICE_STATUS,
  type DeviceStatus,
} from "./status";
import { MAX_WRAP_JITTER_SECS } from "./request";

const SEAL = 13;
const GIFT_WRAP = 1059;
const MARKER: [string, string] = ["t", "charter-device"];

function sample(machine: string, subject: string): DeviceStatus {
  return {
    v: 1,
    subject,
    machine,
    ts: 1_700_000_000,
    dayKey: "2026-07-02",
    usedTodaySecs: 600,
    windowLeftSecs: 1800,
    quotaLeftSecs: 900,
    effectiveSecs: 900,
    locked: false,
    source: "guardian",
    // `parseStatus` always fills these four in (defaults false/0), so a
    // fixture compared against its own round trip must carry them too.
    pausedByAdmin: false,
    enforcementGapSecs: 0,
    relayUnreachablePolls: 0,
    transportUnavailable: false,
  };
}

/** Mirror the device's `emit_status`: a bare machine rumor → seal → gift-wrap. */
function giftWrapStatus(
  status: DeviceStatus,
  machineSk: Uint8Array,
  guardianPk: string,
  createdAt = 1_700_000_000,
) {
  const machinePk = getPublicKey(machineSk);
  const rumor = {
    kind: CHARTER_DEVICE_STATUS,
    pubkey: machinePk,
    created_at: createdAt,
    tags: [MARKER],
    content: JSON.stringify(status),
  };
  const seal = finalizeEvent(
    {
      kind: SEAL,
      created_at: createdAt,
      tags: [],
      content: nip44.encrypt(
        JSON.stringify(rumor),
        nip44.getConversationKey(machineSk, guardianPk),
      ),
    },
    machineSk,
  );
  const eph = generateSecretKey();
  const wrapContent = nip44.encrypt(
    JSON.stringify(seal),
    nip44.getConversationKey(eph, guardianPk),
  );
  return finalizeEvent(
    {
      kind: GIFT_WRAP,
      created_at: createdAt,
      tags: [["p", guardianPk]],
      content: wrapContent,
    },
    eph,
  );
}

/**
 * Forge a STATUS as if from `claimedMachinePk`, but seal it with `attackerSk`
 * (a key the attacker actually holds). The rumor.pubkey is set to the claimed
 * victim key while the seal is signed by the attacker — the exact shape a
 * seal-signature + author-binding check must reject.
 */
function forgeStatusAs(
  status: DeviceStatus,
  claimedMachinePk: string,
  attackerSk: Uint8Array,
  guardianPk: string,
  createdAt = 1_700_000_000,
) {
  const rumor = {
    kind: CHARTER_DEVICE_STATUS,
    pubkey: claimedMachinePk, // lie: not the seal signer
    created_at: createdAt,
    tags: [MARKER],
    content: JSON.stringify(status),
  };
  // Seal is signed by the ATTACKER, encrypted to the guardian with the
  // attacker's own conversation key (symmetric ECDH — decrypts fine guardian-side).
  const seal = finalizeEvent(
    {
      kind: SEAL,
      created_at: createdAt,
      tags: [],
      content: nip44.encrypt(
        JSON.stringify(rumor),
        nip44.getConversationKey(attackerSk, guardianPk),
      ),
    },
    attackerSk,
  );
  const eph = generateSecretKey();
  const wrapContent = nip44.encrypt(
    JSON.stringify(seal),
    nip44.getConversationKey(eph, guardianPk),
  );
  return finalizeEvent(
    {
      kind: GIFT_WRAP,
      created_at: createdAt,
      tags: [["p", guardianPk]],
      content: wrapContent,
    },
    eph,
  );
}

describe("parseStatus", () => {
  it("accepts a well-formed payload", () => {
    const s = sample("ab".repeat(32), "cd".repeat(32));
    expect(parseStatus(JSON.stringify(s))).toEqual(s);
  });

  it("rejects bad version / hex / enum / negative / non-JSON", () => {
    const base = sample("ab".repeat(32), "cd".repeat(32));
    expect(parseStatus(JSON.stringify({ ...base, v: 2 }))).toBeNull();
    expect(parseStatus(JSON.stringify({ ...base, subject: "xyz" }))).toBeNull();
    expect(parseStatus(JSON.stringify({ ...base, source: "nope" }))).toBeNull();
    expect(parseStatus(JSON.stringify({ ...base, effectiveSecs: -1 }))).toBeNull();
    expect(parseStatus(JSON.stringify({ ...base, locked: "yes" }))).toBeNull();
    expect(parseStatus("not json")).toBeNull();
  });
});

// 03-G5/04-G6/B4/relay-health follow-up: the four Linux-runtime fields default
// to false/0 rather than staying undefined, unlike every other absent-not-zero
// STATUS meter — a quiet feed and an explicit "nothing wrong" must both read
// as the same reassuring state here.
describe("the four Linux-runtime STATUS fields", () => {
  it("default to false/0 when absent from the wire", () => {
    const s = sample("ab".repeat(32), "cd".repeat(32));
    const parsed = parseStatus(JSON.stringify(s));
    expect(parsed?.pausedByAdmin).toBe(false);
    expect(parsed?.enforcementGapSecs).toBe(0);
    expect(parsed?.relayUnreachablePolls).toBe(0);
    expect(parsed?.transportUnavailable).toBe(false);
  });

  it("carries the reported values through when set", () => {
    const s = {
      ...sample("ab".repeat(32), "cd".repeat(32)),
      pausedByAdmin: true,
      enforcementGapSecs: 14_400,
      relayUnreachablePolls: 3,
      transportUnavailable: true,
    };
    const parsed = parseStatus(JSON.stringify(s));
    expect(parsed?.pausedByAdmin).toBe(true);
    expect(parsed?.enforcementGapSecs).toBe(14_400);
    expect(parsed?.relayUnreachablePolls).toBe(3);
    expect(parsed?.transportUnavailable).toBe(true);
  });

  it("drops a negative or non-numeric count rather than trusting it", () => {
    const base = sample("ab".repeat(32), "cd".repeat(32));
    const neg = parseStatus(JSON.stringify({ ...base, enforcementGapSecs: -5 }));
    expect(neg?.enforcementGapSecs).toBe(0);
    const nonNum = parseStatus(JSON.stringify({ ...base, relayUnreachablePolls: "3" }));
    expect(nonNum?.relayUnreachablePolls).toBe(0);
  });
});

// Honest attribution (charterd >= 0.7.5): the counter's meaning is a FLOOR,
// never a total, and its absence must never read as a confident zero — a
// device that hasn't reported anything unrecognised is not the same as a
// device that has reported nothing unrecognised.
describe("unrecognisedTodaySecs", () => {
  it("parses a present, non-negative value", () => {
    const s = { ...sample("ab".repeat(32), "cd".repeat(32)), unrecognisedTodaySecs: 11_520 };
    expect(parseStatus(JSON.stringify(s))?.unrecognisedTodaySecs).toBe(11_520);
  });

  it("stays undefined when absent — never coerced to 0", () => {
    const s = sample("ab".repeat(32), "cd".repeat(32));
    expect(parseStatus(JSON.stringify(s))?.unrecognisedTodaySecs).toBeUndefined();
  });

  it("drops a negative or non-numeric value rather than trusting it", () => {
    const base = sample("ab".repeat(32), "cd".repeat(32));
    expect(parseStatus(JSON.stringify({ ...base, unrecognisedTodaySecs: -5 }))?.unrecognisedTodaySecs).toBeUndefined();
    expect(parseStatus(JSON.stringify({ ...base, unrecognisedTodaySecs: "lots" }))?.unrecognisedTodaySecs).toBeUndefined();
  });
});

// Out-of-hours use (spec 2026-08-03): Android-only, additive, and absent
// (never a zero) until the device actually reports it — same parse posture
// as `unrecognisedTodaySecs` above, its Linux-only mirror image.
describe("out-of-hours counters", () => {
  it("parses present, non-negative values", () => {
    const s = {
      ...sample("ab".repeat(32), "cd".repeat(32)),
      outOfHoursTodaySecs: 600,
      outOfHoursWeekSecs: 900,
      outOfHoursNightsWeek: 3,
    };
    const parsed = parseStatus(JSON.stringify(s));
    expect(parsed?.outOfHoursTodaySecs).toBe(600);
    expect(parsed?.outOfHoursWeekSecs).toBe(900);
    expect(parsed?.outOfHoursNightsWeek).toBe(3);
  });

  it("stays undefined when absent — never coerced to 0 (an older ward that never sends them)", () => {
    const s = sample("ab".repeat(32), "cd".repeat(32));
    const parsed = parseStatus(JSON.stringify(s));
    expect(parsed?.outOfHoursTodaySecs).toBeUndefined();
    expect(parsed?.outOfHoursWeekSecs).toBeUndefined();
    expect(parsed?.outOfHoursNightsWeek).toBeUndefined();
  });

  it("drops negative or non-numeric values rather than trusting them", () => {
    const base = sample("ab".repeat(32), "cd".repeat(32));
    const parsed = parseStatus(
      JSON.stringify({ ...base, outOfHoursTodaySecs: -5, outOfHoursWeekSecs: "lots", outOfHoursNightsWeek: -1 }),
    );
    expect(parsed?.outOfHoursTodaySecs).toBeUndefined();
    expect(parsed?.outOfHoursWeekSecs).toBeUndefined();
    expect(parsed?.outOfHoursNightsWeek).toBeUndefined();
  });
});

// The safe-mode tamper signal (S1): boots the device went through with no
// warden running. Absent — never a zero — because a "0 unwarded boots" line
// on every device card would be noise, and worse, would train a guardian to
// scroll past the one that isn't zero.
describe("enforcementGap", () => {
  const base = () => sample("ab".repeat(32), "cd".repeat(32));

  it("parses a reported gap", () => {
    const parsed = parseStatus(
      JSON.stringify({ ...base(), enforcementGap: { unexplainedBoots: 2, lastNoticedAt: 1_700_000_500 } }),
    );
    expect(parsed?.enforcementGap).toEqual({ unexplainedBoots: 2, lastNoticedAt: 1_700_000_500 });
  });

  it("stays undefined when absent — an older ward, or a phone that never booted unwarded", () => {
    expect(parseStatus(JSON.stringify(base()))?.enforcementGap).toBeUndefined();
  });

  it("treats an explicit zero as no gap at all", () => {
    const parsed = parseStatus(
      JSON.stringify({ ...base(), enforcementGap: { unexplainedBoots: 0, lastNoticedAt: 1_700_000_500 } }),
    );
    expect(parsed?.enforcementGap).toBeUndefined();
  });

  it("drops a malformed or partial gap rather than half-trusting it", () => {
    for (const g of [
      { unexplainedBoots: -1, lastNoticedAt: 1 },
      { unexplainedBoots: "two", lastNoticedAt: 1 },
      { unexplainedBoots: 2 },
      { lastNoticedAt: 1 },
      "safe mode!",
      null,
    ]) {
      expect(parseStatus(JSON.stringify({ ...base(), enforcementGap: g }))?.enforcementGap).toBeUndefined();
    }
  });
});

describe("AppRef.userInstalled", () => {
  const withApps = (apps: unknown) => ({ ...sample("ab".repeat(32), "cd".repeat(32)), apps });

  it("parses true only when explicitly true", () => {
    const parsed = parseStatus(
      JSON.stringify(withApps([{ pkg: "org.prismlauncher.PrismLauncher", label: "Prism", userInstalled: true }])),
    );
    expect(parsed?.apps).toEqual([
      { pkg: "org.prismlauncher.PrismLauncher", label: "Prism", userInstalled: true },
    ]);
  });

  it("stays undefined (never false) when absent — root-owned is the unflagged default", () => {
    const parsed = parseStatus(JSON.stringify(withApps([{ pkg: "org.mozilla.firefox", label: "Firefox" }])));
    expect(parsed?.apps?.[0].userInstalled).toBeUndefined();
    expect(parsed?.apps?.[0]).not.toHaveProperty("userInstalled");
  });

  it("never coerces a truthy-but-not-true value to true", () => {
    const parsed = parseStatus(
      JSON.stringify(withApps([{ pkg: "org.mozilla.firefox", label: "Firefox", userInstalled: 1 }])),
    );
    expect(parsed?.apps?.[0].userInstalled).toBeUndefined();
  });
});

/**
 * A device the guardian has removed apps from keeps REPORTING them, flagged —
 * the inventory is the only surface a removal can be undone from, so an app
 * that vanished from the tablet AND the guardian's list would be gone for
 * good. Parsed with exactly `userInstalled`'s rules above (true or absent,
 * never coerced), so an old warden that reports neither reads the same as it
 * always did.
 */
describe("AppRef.hidden", () => {
  const withApps = (apps: unknown) => ({ ...sample("ab".repeat(32), "cd".repeat(32)), apps });

  it("parses true only when explicitly true", () => {
    const parsed = parseStatus(
      JSON.stringify(withApps([{ pkg: "com.sec.android.app.samsungapps", label: "Galaxy Store", hidden: true }])),
    );
    expect(parsed?.apps).toEqual([
      { pkg: "com.sec.android.app.samsungapps", label: "Galaxy Store", hidden: true },
    ]);
  });

  it("stays undefined (never false) when absent — an ordinary app is unflagged", () => {
    const parsed = parseStatus(JSON.stringify(withApps([{ pkg: "org.mozilla.firefox", label: "Firefox" }])));
    expect(parsed?.apps?.[0].hidden).toBeUndefined();
    expect(parsed?.apps?.[0]).not.toHaveProperty("hidden");
  });

  it("never coerces a truthy-but-not-true value to true", () => {
    const parsed = parseStatus(
      JSON.stringify(withApps([{ pkg: "org.mozilla.firefox", label: "Firefox", hidden: "yes" }])),
    );
    expect(parsed?.apps?.[0].hidden).toBeUndefined();
  });

  it("carries both flags at once — a hidden ward-writable entry is still flagged as one", () => {
    const parsed = parseStatus(
      JSON.stringify(
        withApps([{ pkg: "org.prismlauncher.PrismLauncher", label: "Prism", userInstalled: true, hidden: true }]),
      ),
    );
    expect(parsed?.apps).toEqual([
      { pkg: "org.prismlauncher.PrismLauncher", label: "Prism", userInstalled: true, hidden: true },
    ]);
  });
});

describe("unwrapStatus", () => {
  it("round-trips a device-emitted STATUS to the guardian", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const s = sample(getPublicKey(machineSk), "cd".repeat(32));
    const wrap = giftWrapStatus(s, machineSk, guardianPk);
    expect(unwrapStatus(wrap, guardianSk, 1_700_000_000)).toEqual(s);
  });

  it("returns null for the wrong recipient", () => {
    const machineSk = generateSecretKey();
    const guardianPk = getPublicKey(generateSecretKey());
    const s = sample(getPublicKey(machineSk), "cd".repeat(32));
    const wrap = giftWrapStatus(s, machineSk, guardianPk);
    expect(unwrapStatus(wrap, generateSecretKey())).toBeNull();
  });

  it("rejects spoofed attribution (payload.machine != rumor author)", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    // machine field is a different key than the actual emitter (machineSk).
    const s = sample("ab".repeat(32), "cd".repeat(32));
    const wrap = giftWrapStatus(s, machineSk, guardianPk);
    expect(unwrapStatus(wrap, guardianSk)).toBeNull();
  });

  it("rejects a forged status attributed to a device the attacker doesn't hold", () => {
    // The core spoof: nip44 sealing alone is symmetric, so an attacker who knows
    // only the guardian's PUBLIC key can seal a payload to it with their own
    // keypair and set rumor.pubkey + payload.machine to a VICTIM device's key.
    // Without a seal-signature + author-binding check this was accepted as the
    // victim's live state (lock-state spoof / QR-pairing hijack).
    const attackerSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const victimPk = "ab".repeat(32); // a real device key the attacker can't sign for
    const s = sample(victimPk, "cd".repeat(32));
    const wrap = forgeStatusAs(s, victimPk, attackerSk, guardianPk);
    expect(unwrapStatus(wrap, guardianSk)).toBeNull();
  });

  it("drops a stale or future-dated wrap (replay window)", () => {
    const machineSk = generateSecretKey();
    const guardianSk = generateSecretKey();
    const guardianPk = getPublicKey(guardianSk);
    const s = sample(getPublicKey(machineSk), "cd".repeat(32));
    const NOW = 1_700_100_000;
    const fresh = giftWrapStatus(s, machineSk, guardianPk, NOW - MAX_WRAP_JITTER_SECS);
    expect(unwrapStatus(fresh, guardianSk, NOW)).toEqual(s);
    const stale = giftWrapStatus(s, machineSk, guardianPk, NOW - MAX_WRAP_JITTER_SECS - 1);
    expect(unwrapStatus(stale, guardianSk, NOW)).toBeNull();
    const future = giftWrapStatus(s, machineSk, guardianPk, NOW + MAX_WRAP_JITTER_SECS + 1);
    expect(unwrapStatus(future, guardianSk, NOW)).toBeNull();
  });
});

describe("pairToken (QR onboarding echo)", () => {
  it("accepts a well-formed token and drops garbage", () => {
    const base: Record<string, unknown> = {
      ...sample("ab".repeat(32), "cd".repeat(32)),
    };
    base.pairToken = "0f3a9c21b4d8e6a7550f3a9c21b4d8e6";
    const ok = parseStatus(JSON.stringify(base));
    expect(ok?.pairToken).toBe("0f3a9c21b4d8e6a7550f3a9c21b4d8e6");

    base.pairToken = "not a token!!";
    const bad = parseStatus(JSON.stringify(base));
    expect(bad).not.toBeNull();
    expect(bad?.pairToken).toBeUndefined();

    delete base.pairToken;
    expect(parseStatus(JSON.stringify(base))?.pairToken).toBeUndefined();
  });
});

describe("the install-window account", () => {
  const base = () =>
    sample("ab".repeat(32), "cd".repeat(32)) as unknown as Record<string, unknown>;
  const parse = (installWindow: unknown) =>
    parseStatus(JSON.stringify({ ...base(), installWindow }))?.installWindow;

  it("carries a real account through", () => {
    const account = {
      startedAt: 1_782_752_400,
      endedAt: 1_782_754_200,
      changes: [
        { pkg: "com.example.game", label: "A Game", kind: "installed", at: 1_782_752_500 },
        { pkg: "com.example.chat", label: "Chat", kind: "updated", at: 1_782_752_600 },
      ],
    };
    expect(parse(account)).toEqual(account);
  });

  it("keeps an open window's account, with no end yet", () => {
    const open = parse({ startedAt: 1_782_752_400, changes: [] });
    expect(open?.endedAt).toBeUndefined();
    expect(open?.changes).toEqual([]);
  });

  /**
   * The distinction the whole feature rests on: a MALFORMED account must read
   * as "no signal", never as "nothing was installed". The empty answer is the
   * one a guardian acts on, so it may only ever come from a phone that actually
   * said so.
   */
  it("is absent, not empty, when it cannot be read", () => {
    expect(parse({ changes: [] })).toBeUndefined(); // no startedAt
    expect(parse({ startedAt: 1, changes: "lots" })).toBeUndefined();
    expect(parse("nonsense")).toBeUndefined();
    expect(parse(null)).toBeUndefined();
  });

  it("drops individual entries it cannot trust, keeping the ones it can", () => {
    const account = parse({
      startedAt: 1_782_752_400,
      changes: [
        { pkg: "com.ok", label: "Fine", kind: "installed", at: 1_782_752_500 },
        { pkg: "com.bad", label: "Bad kind", kind: "sideloaded", at: 1_782_752_500 },
        { pkg: "", label: "No package", kind: "installed", at: 1_782_752_500 },
        { pkg: "com.bad2", label: "No time", kind: "updated", at: -5 },
      ],
    });
    expect(account?.changes.map((c) => c.pkg)).toEqual(["com.ok"]);
  });

  it("falls back to the package name when the label is unusable", () => {
    const account = parse({
      startedAt: 1,
      changes: [{ pkg: "com.example.app", label: "", kind: "installed", at: 2 }],
    });
    expect(account?.changes[0].label).toBe("com.example.app");
  });

  it("caps a hostile list rather than rendering it", () => {
    const changes = Array.from({ length: 500 }, (_, i) => ({
      pkg: `com.example.p${i}`,
      label: `P${i}`,
      kind: "installed",
      at: 1_782_752_400,
    }));
    expect(parse({ startedAt: 1, changes })?.changes).toHaveLength(200);
  });
});

describe("groups (named-times bucket progress)", () => {
  const base = () =>
    sample("ab".repeat(32), "cd".repeat(32)) as unknown as Record<string, unknown>;
  const parse = (groups: unknown) => parseStatus(JSON.stringify({ ...base(), groups }))?.groups;

  it("carries the contract.md worked example through", () => {
    expect(parse([{ id: "play", daySecs: 1320, weekSecs: 4800 }])).toEqual([
      { id: "play", daySecs: 1320, weekSecs: 4800 },
    ]);
  });

  it("is absent (not empty) when the field is missing — matches every other STATUS heartbeat", () => {
    expect(parseStatus(JSON.stringify(base()))?.groups).toBeUndefined();
  });

  it("drops individual malformed entries, keeping the ones it can trust", () => {
    const out = parse([
      { id: "play", daySecs: 60, weekSecs: 600 },
      { id: "", daySecs: 60, weekSecs: 600 }, // blank id
      { id: "social", daySecs: -1, weekSecs: 600 }, // negative
      { id: "chat", daySecs: 60, weekSecs: "lots" }, // wrong type
      "nonsense",
    ]);
    expect(out?.map((g) => g.id)).toEqual(["play"]);
  });

  it("is undefined, not empty, when nothing in the list can be trusted", () => {
    expect(parse([{ id: "" }])).toBeUndefined();
    expect(parse("not an array")).toBeUndefined();
    expect(parse(null)).toBeUndefined();
  });

  /** Cap mirrors `charter-schedule::MAX_BUCKETS` (12). A payload claiming more
   *  is TRUNCATED on parse, not rejected — losing an otherwise-trustworthy
   *  STATUS over a malformed tail would hide real numbers the guardian is owed. */
  it("truncates a hostile list at 12 rather than rejecting the whole payload", () => {
    const groups = Array.from({ length: 20 }, (_, i) => ({
      id: `g${i}`,
      daySecs: i,
      weekSecs: i,
    }));
    expect(parse(groups)).toHaveLength(12);
    expect(parse(groups)?.map((g) => g.id)).toEqual(
      Array.from({ length: 12 }, (_, i) => `g${i}`),
    );
  });
});
