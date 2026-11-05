import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { GUARDIAN_KEYS_MIN_VERSION, childReadiness, wardReadiness } from "./versionGate";
import { parsePresenceCode, parsePresenceToken } from "./presenceCode";
import { keyStanding, parseStatusGuardians } from "./statusGuardians";
import { guardianPinHash, makeGuardianPinRecord, pinOnlyRelaysTemplate } from "./pin";
import { validateGuardianPinRecord } from "./sdk";
import { parseStatus } from "../wire/status";

const R = "a1".repeat(32);
const W = "b2".repeat(32);
const M = "c3".repeat(32);
const S = "d4".repeat(32);
const TOKEN = "0123456789abcdef0123456789abcdef";

describe("versioning gate", () => {
  it("holds the per-platform thresholds", () => {
    expect(GUARDIAN_KEYS_MIN_VERSION).toEqual({ android: 55, linux: 804 });
  });

  it("is ready at and above the threshold, per platform", () => {
    expect(wardReadiness("android", 55, true)).toEqual({ ready: true });
    expect(wardReadiness("linux", 804, true)).toEqual({ ready: true });
    expect(wardReadiness("linux", 900, true)).toEqual({ ready: true });
    // The last released builds, which predate the section, are not ready.
    expect(wardReadiness("android", 54, true)).toMatchObject({ ready: false, reason: "too-old", needs: 55 });
    expect(wardReadiness("linux", 803, true)).toMatchObject({ ready: false, reason: "too-old" });
    // Numbering is per platform: an Android 803 would be far above.
    expect(wardReadiness("android", 803, true)).toEqual({ ready: true });
  });

  it("counts a ward that has never sent STATUS as not ready", () => {
    expect(wardReadiness("android", undefined, false)).toEqual({ ready: false, reason: "silent" });
    // Even a remembered version is not trusted without a STATUS read here.
    expect(wardReadiness("android", 99, false)).toEqual({ ready: false, reason: "silent" });
  });

  it("fails closed on a STATUS with no version or an unknown platform", () => {
    expect(wardReadiness("android", undefined, true)).toEqual({ ready: false, reason: "no-version" });
    expect(wardReadiness("windows", 9999, true)).toEqual({ ready: false, reason: "unknown-platform" });
    expect(wardReadiness(undefined, 9999, true)).toEqual({ ready: false, reason: "unknown-platform" });
  });

  it("needs every ward of a child, and at least one", () => {
    expect(childReadiness([]).ready).toBe(false);
    const ok = { machine: M, platform: "android", appVersionCode: 55, statusSeen: true };
    expect(childReadiness([ok]).ready).toBe(true);
    const silent = { machine: S, platform: "linux", statusSeen: false };
    const r = childReadiness([ok, silent]);
    expect(r.ready).toBe(false);
    expect(r.wards[1].readiness).toEqual({ ready: false, reason: "silent" });
  });
});

describe("presence code", () => {
  const d = `kintrinsic/guardian/${R}`;
  const good = `charter://presence?m=${M}&t=${TOKEN}&d=${encodeURIComponent(d)}&s=${S}`;

  it("parses the build-defined code", () => {
    expect(parsePresenceCode(good)).toEqual({
      machine: M,
      token: TOKEN,
      d,
      kind: "coguardian",
      key: R,
      statement: S,
    });
    expect(parsePresenceCode(`  ${good}\n`)?.kind).toBe("coguardian");
    const removal = `charter://presence?m=${M}&t=${TOKEN}&d=kintrinsic%2Funguardian%2F${R}&s=${S}`;
    expect(parsePresenceCode(removal)).toMatchObject({ kind: "removal", key: R });
    const move = `charter://presence?m=${M}&t=${TOKEN}&d=kintrinsic/root/${R}&s=${S}`;
    expect(parsePresenceCode(move)).toMatchObject({ kind: "move", key: R });
  });

  it("refuses bad input", () => {
    const bad = [
      "",
      "garbage",
      `charter://pair?m=${M}&t=${TOKEN}`,
      good.replace(`m=${M}`, `m=${M.slice(2)}`),
      good.replace(`m=${M}`, `m=${M.toUpperCase()}`),
      good.replace(`t=${TOKEN}`, `t=${TOKEN}00`),
      good.replace(`&s=${S}`, ""),
      good.replace(`s=${S}`, `s=${S.slice(1)}z`),
      `${good}&m=${M}`, // repeated parameter
      // Not a pending statement's d: an approval, a cancel, a bad key.
      good.replace(encodeURIComponent(d), encodeURIComponent(`kintrinsic/device/${R}`)),
      good.replace(encodeURIComponent(d), encodeURIComponent(`kintrinsic/cancel/${d}`)),
      good.replace(encodeURIComponent(d), encodeURIComponent(`kintrinsic/guardian/${R.slice(2)}`)),
    ];
    for (const b of bad) expect(parsePresenceCode(b)).toBeNull();
  });

  it("parses the typed short form in groups", () => {
    expect(parsePresenceToken("01234567 89ABCDEF 01234567-89abcdef")).toBe(TOKEN);
    expect(parsePresenceToken("0123")).toBeNull();
  });
});

describe("STATUS guardians", () => {
  const g = {
    roots: [{ root: R, keys: [W] }],
    pending: [{ d: `kintrinsic/unguardian/${R}`, kind: "removal", takesEffectAt: 9, expiresAt: 10 }],
    tombstones: 2,
    relays: ["wss://nos.lol"],
  };

  it("parses the contract shape", () => {
    expect(parseStatusGuardians(g)).toEqual(g);
    expect(keyStanding(g as never, W)).toEqual({ as: "working", root: R });
    expect(keyStanding(g as never, R)).toEqual({ as: "root" });
    expect(keyStanding(g as never, M)).toEqual({ as: "none" });
  });

  it("drops the whole object when anything is malformed", () => {
    expect(parseStatusGuardians(undefined)).toBeUndefined();
    expect(parseStatusGuardians({ ...g, tombstones: -1 })).toBeUndefined();
    expect(parseStatusGuardians({ ...g, roots: [{ root: "xx", keys: [] }] })).toBeUndefined();
    expect(parseStatusGuardians({ ...g, roots: [{ root: R, keys: ["nope"] }] })).toBeUndefined();
    // kind must agree with the d.
    expect(parseStatusGuardians({ ...g, pending: [{ ...g.pending[0], kind: "move" }] })).toBeUndefined();
    expect(parseStatusGuardians({ ...g, pending: [{ ...g.pending[0], expiresAt: "10" }] })).toBeUndefined();
    expect(parseStatusGuardians({ ...g, pending: [{ ...g.pending[0], signer: "nope" }] })).toBeUndefined();
  });

  it("keeps the pending statement's signer where the ward reports it", () => {
    const withSigner = { ...g, pending: [{ ...g.pending[0], signer: W }] };
    expect(parseStatusGuardians(withSigner)).toEqual(withSigner);
  });

  it("rides on a parsed STATUS, with appVersionCode", () => {
    const base = {
      v: 1,
      subject: M,
      machine: M,
      ts: 1,
      dayKey: "2026-10-29",
      usedTodaySecs: 0,
      windowLeftSecs: 0,
      quotaLeftSecs: 0,
      effectiveSecs: 0,
      locked: false,
      source: "guardian",
      appVersionCode: 804,
    };
    const st = parseStatus(JSON.stringify({ ...base, guardians: g }));
    expect(st?.guardians).toEqual(g);
    expect(st?.appVersionCode).toBe(804);
    // An older ward: no guardians, the STATUS is still good.
    expect(parseStatus(JSON.stringify(base))?.guardians).toBeUndefined();
    // A malformed guardians block degrades to absent, never drops the STATUS.
    expect(parseStatus(JSON.stringify({ ...base, guardians: { roots: 1 } }))?.locked).toBe(false);
  });
});

describe("guardian PIN", () => {
  const vectors = JSON.parse(
    readFileSync(resolve(__dirname, "../../../../core/crates/charter-testkit/vectors/guardians/pin.json"), "utf8"),
  ) as { vectors: { pin: string; salt: string; hash: string; record: unknown }[] };

  it("matches every golden vector", async () => {
    expect(vectors.vectors.length).toBeGreaterThan(0);
    for (const v of vectors.vectors) {
      expect(await guardianPinHash(v.pin, v.salt)).toBe(v.hash);
      // The app itself asks for at least 6 digits; the derivation is the same.
      if (v.pin.length >= 6) expect(await makeGuardianPinRecord(v.pin, v.salt)).toEqual(v.record);
    }
  }, 30_000);

  it("round-trips the test PIN 123456 with a fresh 16-byte salt", async () => {
    const rec = await makeGuardianPinRecord("123456");
    expect(rec.salt).toMatch(/^[0-9a-f]{32}$/);
    expect(validateGuardianPinRecord(rec)).toEqual({ ok: true, record: rec });
    expect(await guardianPinHash("123456", rec.salt)).toBe(rec.hash);
    expect(await guardianPinHash("123457", rec.salt)).not.toBe(rec.hash);
  }, 30_000);

  it("refuses a short or non-numeric PIN", async () => {
    await expect(makeGuardianPinRecord("12345")).rejects.toThrow(/6 digits/);
    await expect(makeGuardianPinRecord("12345a")).rejects.toThrow(/6 digits/);
  });

  it("puts the record in a PIN-only RELAYS body: no relays field (D4)", () => {
    const rec = { v: 1 as const, salt: "00".repeat(16), hash: "11".repeat(32) };
    const t = pinOnlyRelaysTemplate(M, rec, 5);
    expect(t.kind).toBe(31118);
    expect(t.tags).toEqual([["t", "charter-device"], ["p", M]]);
    expect(JSON.parse(t.content)).toEqual({ machine: M, guardianPin: rec });
  });
});
