import { describe, expect, it } from "vitest";
import {
  finalizeEvent,
  generateSecretKey,
  getPublicKey,
  nip44,
  type NostrEvent,
} from "nostr-tools";
import {
  CHARTER_DEVICE_REQUEST,
  MAX_WRAP_JITTER_SECS,
  parseRequest,
  unwrapRequest,
  type DeviceRequest,
} from "./request";

const SEAL = 13;
const GIFT_WRAP = 1059;
const MARKER: [string, string] = ["t", "charter-device"];
const NOW = 1_700_000_000;

function sample(machine: string): DeviceRequest {
  return {
    v: 1,
    op: "time.extend",
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine,
    ts: NOW,
    params: { minutesRequested: 30, reason: "homework's done", limitHit: "budget" },
  };
}

// Mirror the device's wrap: bare rumor, machine-signed seal, ephemeral 1059.
function giftWrapRequest(
  request: DeviceRequest,
  machineSk: Uint8Array,
  guardianPk: string,
  wrapCreatedAt: number,
): NostrEvent {
  const rumor = {
    kind: CHARTER_DEVICE_REQUEST,
    pubkey: getPublicKey(machineSk),
    created_at: request.ts,
    tags: [MARKER],
    content: JSON.stringify(request),
  };
  const seal = finalizeEvent(
    {
      kind: SEAL,
      created_at: request.ts,
      tags: [],
      content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(machineSk, guardianPk)),
    },
    machineSk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    {
      kind: GIFT_WRAP,
      created_at: wrapCreatedAt,
      tags: [["p", guardianPk]],
      content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, guardianPk)),
    },
    eph,
  );
}

describe("unwrapRequest wrap freshness (mirrors charter-transport MAX_JITTER_SECS)", () => {
  const machineSk = generateSecretKey();
  const guardianSk = generateSecretKey();
  const guardianPk = getPublicKey(guardianSk);
  const r = sample(getPublicKey(machineSk));

  it("accepts a wrap stamped now, and one exactly at the jitter bound", () => {
    const fresh = giftWrapRequest(r, machineSk, guardianPk, NOW);
    expect(unwrapRequest(fresh, guardianSk, NOW)).toEqual(r);
    // NIP-59 wraps are backdated up to the same window — the boundary is legal.
    const edge = giftWrapRequest(r, machineSk, guardianPk, NOW - MAX_WRAP_JITTER_SECS);
    expect(unwrapRequest(edge, guardianSk, NOW)).toEqual(r);
  });

  it("drops a wrap older than the jitter bound (replayed stale ask)", () => {
    const stale = giftWrapRequest(r, machineSk, guardianPk, NOW - MAX_WRAP_JITTER_SECS - 1);
    expect(unwrapRequest(stale, guardianSk, NOW)).toBeNull();
  });

  it("drops a wrap from the future beyond the jitter bound", () => {
    const future = giftWrapRequest(r, machineSk, guardianPk, NOW + MAX_WRAP_JITTER_SECS + 1);
    expect(unwrapRequest(future, guardianSk, NOW)).toBeNull();
  });
});

describe("parseRequest — install.apk", () => {
  const base = {
    v: 1,
    op: "install.apk",
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine: "12".repeat(32),
    ts: 1_700_000_000,
  };

  it("accepts a well-formed staged install ask", () => {
    const r = parseRequest(
      JSON.stringify({
        ...base,
        params: { packageName: "org.mozilla.fenix", label: "Firefox", source: "staged" },
      }),
    );
    expect(r?.op).toBe("install.apk");
    if (r?.op === "install.apk") {
      expect(r.params.packageName).toBe("org.mozilla.fenix");
      expect(r.params.label).toBe("Firefox");
    }
  });

  it("drops bad package names, unknown sources, oversize labels (fail-closed)", () => {
    const mk = (params: unknown) => parseRequest(JSON.stringify({ ...base, params }));
    expect(mk({ packageName: "nodots", source: "staged" })).toBeNull();
    expect(mk({ packageName: "org.ok.app", source: "url" })).toBeNull();
    expect(mk({ packageName: "org.ok.app", source: "staged", label: "x".repeat(300) })).toBeNull();
  });
});

describe("parseRequest — time.extend bucket limitHit / bucketId (named times)", () => {
  const base = {
    v: 1,
    op: "time.extend",
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine: "12".repeat(32),
    ts: 1_700_000_000,
  };

  it("accepts limitHit: 'bucket' with a bucketId", () => {
    const r = parseRequest(
      JSON.stringify({
        ...base,
        params: { minutesRequested: 20, reason: "just finishing this build", limitHit: "bucket", bucketId: "play" },
      }),
    );
    expect(r?.op).toBe("time.extend");
    if (r?.op === "time.extend") {
      expect(r.params.limitHit).toBe("bucket");
      expect(r.params.bucketId).toBe("play");
    }
  });

  it("bucketId is optional and absent for whole-device dimensions", () => {
    const r = parseRequest(
      JSON.stringify({ ...base, params: { minutesRequested: 20, limitHit: "budget" } }),
    );
    expect(r?.op).toBe("time.extend");
    if (r?.op === "time.extend") {
      expect(r.params.bucketId).toBeUndefined();
    }
  });

  it("drops a malformed bucketId (fail-closed)", () => {
    const mk = (bucketId: unknown) =>
      parseRequest(
        JSON.stringify({ ...base, params: { minutesRequested: 20, limitHit: "bucket", bucketId } }),
      );
    expect(mk("Bad Id!")).toBeNull();
    expect(mk("x".repeat(41))).toBeNull();
    expect(mk(42)).toBeNull();
  });
});

describe("parseRequest — app.open (named times' on-request apps)", () => {
  const base = {
    v: 1,
    op: "app.open",
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    subject: "ef".repeat(32),
    machine: "12".repeat(32),
    ts: 1_700_000_000,
  };

  it("accepts a well-formed ask (pkg only)", () => {
    const r = parseRequest(
      JSON.stringify({ ...base, params: { pkg: "com.mojang.minecraftpe" } }),
    );
    expect(r?.op).toBe("app.open");
    if (r?.op === "app.open") {
      expect(r.params.pkg).toBe("com.mojang.minecraftpe");
      expect(r.params.label).toBeUndefined();
      expect(r.params.minutesRequested).toBeUndefined();
      expect(r.params.reason).toBeUndefined();
    }
  });

  it("accepts the full contract.md worked example", () => {
    const r = parseRequest(
      JSON.stringify({
        ...base,
        params: {
          pkg: "com.mojang.minecraftpe",
          label: "Minecraft",
          minutesRequested: 30,
          reason: "almost done building",
        },
      }),
    );
    expect(r?.op).toBe("app.open");
    if (r?.op === "app.open") {
      expect(r.params).toEqual({
        pkg: "com.mojang.minecraftpe",
        label: "Minecraft",
        minutesRequested: 30,
        reason: "almost done building",
      });
    }
  });

  it("accepts a Linux exec-path identity (not just a dotted Android package)", () => {
    const r = parseRequest(
      JSON.stringify({ ...base, params: { pkg: "/usr/games/supertux2" } }),
    );
    expect(r?.op).toBe("app.open");
    if (r?.op === "app.open") expect(r.params.pkg).toBe("/usr/games/supertux2");
  });

  it("rejects an empty/blank pkg (fail-closed — nothing to open)", () => {
    const mk = (pkg: unknown) => parseRequest(JSON.stringify({ ...base, params: { pkg } }));
    expect(mk("")).toBeNull();
    expect(mk("   ")).toBeNull();
    expect(mk(42)).toBeNull();
    expect(mk(undefined)).toBeNull();
  });

  it("rejects minutesRequested out of 1..=1440, exactly like time.extend", () => {
    const mk = (minutesRequested: unknown) =>
      parseRequest(JSON.stringify({ ...base, params: { pkg: "x.y", minutesRequested } }));
    expect(mk(0)).toBeNull();
    expect(mk(1441)).toBeNull();
    expect(mk(1)).not.toBeNull();
    expect(mk(1440)).not.toBeNull();
  });

  it("rejects an oversize label or reason (>280 chars, same bound as time.extend)", () => {
    const mkLabel = (label: unknown) =>
      parseRequest(JSON.stringify({ ...base, params: { pkg: "x.y", label } }));
    const mkReason = (reason: unknown) =>
      parseRequest(JSON.stringify({ ...base, params: { pkg: "x.y", reason } }));
    expect(mkLabel("x".repeat(281))).toBeNull();
    expect(mkLabel("x".repeat(280))).not.toBeNull();
    expect(mkReason("x".repeat(281))).toBeNull();
  });

  it("still enforces the shared reqId/nonce/subject/machine hex discipline", () => {
    const mk = (over: Record<string, unknown>) =>
      parseRequest(JSON.stringify({ ...base, ...over, params: { pkg: "x.y" } }));
    expect(mk({ reqId: "short" })).toBeNull();
    expect(mk({ nonce: "short" })).toBeNull();
    expect(mk({ subject: "not-hex" })).toBeNull();
    expect(mk({ machine: "not-hex" })).toBeNull();
    expect(mk({ v: 2 })).toBeNull();
  });
});
