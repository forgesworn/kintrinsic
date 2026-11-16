// The guardian's read side of the ward's emergency unlock (kind 31000).
import { describe, expect, it } from "vitest";
import { generateSecretKey, getPublicKey, type NostrEvent } from "nostr-tools";
import { unwrapAudit, unwrapDeviceAudit } from "./audit";
import { CHARTER_DEVICE_REQUEST, MAX_WRAP_JITTER_SECS } from "./request";
import { CHARTER_DEVICE_STATUS } from "./status";
import { emergencyCallTags as call, overrideTags as override, overrideWrap, type WrapOpts } from "../test/overrideWrap";

const guardianSk = generateSecretKey();
const guardianPk = getPublicKey(guardianSk);
const machineSk = generateSecretKey();
const machinePk = getPublicKey(machineSk);
const NOW = 1_800_000_000;

const wrapFor = (o: Partial<WrapOpts> = {}): NostrEvent => overrideWrap({ guardianPk, machineSk, createdAt: NOW, ...o });

describe("unwrapAudit", () => {
  it("accepts a good override wrap", () => {
    const w = wrapFor();
    expect(unwrapAudit(w, guardianSk, NOW)).toEqual({
      machine: machinePk,
      scope: "full",
      durationSecs: 300,
      at: NOW,
      atStamped: false,
      key: `wrap:${w.id}`,
    });
  });

  it("prefers the at tag, and keys on it so a rebuilt wrap is the same override", () => {
    const first = wrapFor({ tags: override([["at", String(NOW - 600)]]) });
    const rebuilt = wrapFor({ tags: override([["at", String(NOW - 600)]]), createdAt: NOW + 3600 });
    const a = unwrapAudit(first, guardianSk, NOW)!;
    const b = unwrapAudit(rebuilt, guardianSk, NOW + 3600)!;
    expect(a.at).toBe(NOW - 600);
    expect(a.key).toBe(`${machinePk}:${NOW - 600}`);
    expect(b.key).toBe(a.key);
    expect(first.id).not.toBe(rebuilt.id);
  });

  it("falls back on created_at, keyed by the wrap, when there is no at tag", () => {
    const w = wrapFor({ createdAt: NOW - 90 });
    const a = unwrapAudit(w, guardianSk, NOW)!;
    expect(a.at).toBe(NOW - 90);
    expect(a.key).toBe(`wrap:${w.id}`);
  });

  it.each(["-5", "1e9", " 12", "12.5", "abc", ""])("ignores an unparseable at tag %j", (bad) => {
    const w = wrapFor({ tags: override([["at", bad]]) });
    const a = unwrapAudit(w, guardianSk, NOW)!;
    expect(a.at).toBe(NOW);
    expect(a.key).toBe(`wrap:${w.id}`);
  });

  it("dates by created_at, but still keys on the tag, when at is later than the wrap's own time", () => {
    const w = wrapFor({ tags: override([["at", String(NOW + 1)]]) });
    const a = unwrapAudit(w, guardianSk, NOW)!;
    expect(a.at).toBe(NOW);
    expect(a.key).toBe(`${machinePk}:${NOW + 1}`);
  });

  it("does not date a line 1970 for an at of zero from a reset clock", () => {
    const a = unwrapAudit(wrapFor({ tags: override([["at", "0"]]) }), guardianSk, NOW)!;
    expect(a.at).toBe(NOW);
    expect(a.key).toBe(`${machinePk}:0`);
  });

  it("uses an at up to 30 days old, and not beyond", () => {
    const day = 24 * 60 * 60;
    const wide = { wrapAt: NOW }; // the wrap itself stays inside the jitter window
    const ok = unwrapAudit(wrapFor({ ...wide, tags: override([["at", String(NOW - 29 * day)]]) }), guardianSk, NOW)!;
    expect(ok.at).toBe(NOW - 29 * day);
    const old = unwrapAudit(wrapFor({ ...wide, tags: override([["at", String(NOW - 31 * day)]]) }), guardianSk, NOW)!;
    expect(old.at).toBe(NOW);
    expect(old.key).toBe(`${machinePk}:${NOW - 31 * day}`);
  });

  it("folds a rebuilt wrap into one override even after the ward's clock stepped back", () => {
    // Pressed at T; the first wrap is built just after. The clock then steps back
    // before the rebuild, so the rebuilt wrap's created_at is EARLIER than `at`.
    const T = NOW - 600;
    const first = wrapFor({ tags: override([["at", String(T)]]), createdAt: T + 15 });
    const rebuilt = wrapFor({ tags: override([["at", String(T)]]), createdAt: T - 3600 });
    const a = unwrapAudit(first, guardianSk, NOW)!;
    const b = unwrapAudit(rebuilt, guardianSk, NOW)!;
    expect(a.key).toBe(b.key);
    expect(a.at).toBe(T);
    expect(b.at).toBe(T - 3600); // dated by its own wrap: the tag is not believed
  });

  it("rejects a seal signed by another key", () => {
    expect(unwrapAudit(wrapFor({ sealSk: generateSecretKey() }), guardianSk, NOW)).toBeNull();
  });

  it("rejects a rumor whose author is not the seal's signer", () => {
    const other = getPublicKey(generateSecretKey());
    expect(unwrapAudit(wrapFor({ rumorPk: other }), guardianSk, NOW)).toBeNull();
  });

  it("rejects a wrap outside the jitter window, either way", () => {
    const w = wrapFor({ wrapAt: NOW - MAX_WRAP_JITTER_SECS - 1 });
    expect(unwrapAudit(w, guardianSk, NOW)).toBeNull();
    const future = wrapFor({ wrapAt: NOW + MAX_WRAP_JITTER_SECS + 1 });
    expect(unwrapAudit(future, guardianSk, NOW)).toBeNull();
    expect(unwrapAudit(wrapFor({ wrapAt: NOW - MAX_WRAP_JITTER_SECS }), guardianSk, NOW)).not.toBeNull();
  });

  it("accepts only outcome=override with op=unlock.breakglass", () => {
    const tags = (outcome: string, op: string) => [
      ["t", "charter-device"],
      ["outcome", outcome],
      ["op", op],
    ];
    expect(unwrapAudit(wrapFor({ tags: tags("enacted", "unlock.breakglass") }), guardianSk, NOW)).toBeNull();
    expect(unwrapAudit(wrapFor({ tags: tags("override", "time.extend") }), guardianSk, NOW)).toBeNull();
    expect(unwrapAudit(wrapFor({ tags: [["t", "charter-device"]] }), guardianSk, NOW)).toBeNull();
  });

  it("is not tripped by the other device kinds", () => {
    expect(unwrapAudit(wrapFor({ kind: CHARTER_DEVICE_STATUS }), guardianSk, NOW)).toBeNull();
    expect(unwrapAudit(wrapFor({ kind: CHARTER_DEVICE_REQUEST }), guardianSk, NOW)).toBeNull();
  });

  it("never throws on garbage", () => {
    const junk = { ...wrapFor(), content: "not nip44" } as NostrEvent;
    expect(unwrapAudit(junk, guardianSk, NOW)).toBeNull();
  });

  it("still accepts an override with an unknown scope or a bad duration, with less detail", () => {
    const odd = wrapFor({
      tags: [
        ["outcome", "override"],
        ["op", "unlock.breakglass"],
        ["scope", "everything"],
        ["durationSecs", "soon"],
      ],
    });
    expect(unwrapAudit(odd, guardianSk, NOW)).toMatchObject({ scope: null, durationSecs: null });
    for (const d of ["0", "-60", "1.5"]) {
      const w = wrapFor({
        tags: [
          ["outcome", "override"],
          ["op", "unlock.breakglass"],
          ["scope", "calls"],
          ["durationSecs", d],
        ],
      });
      expect(unwrapAudit(w, guardianSk, NOW)).toMatchObject({ scope: "calls", durationSecs: null });
    }
  });

  it("reads each tag by name, first match", () => {
    const w = wrapFor({
      tags: [
        ["op", "unlock.breakglass"],
        ["outcome", "override"],
        ["scope", "calls"],
        ["scope", "full"],
        ["durationSecs", "120"],
      ],
    });
    expect(unwrapAudit(w, guardianSk, NOW)).toMatchObject({ scope: "calls", durationSecs: 120 });
  });
});

describe("screen-lock reset audit (op screenlock.reset)", () => {
  const ID = "0123456789abcdef0123456789abcdef";
  const reset = (extra: string[][] = [], outcome = "applied"): string[][] => [
    ["t", "charter-device"],
    ["outcome", outcome],
    ["op", "screenlock.reset"],
    ["id", ID],
    ...extra,
  ];

  it("is accepted, with the ward's id, outcome and at", () => {
    const w = wrapFor({ tags: reset([["at", String(NOW - 30)]]) });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
      op: "screenlock.reset",
      reset: { machine: machinePk, id: ID, result: "applied", at: NOW - 30 },
    });
  });

  it("is still not an emergency unlock", () => {
    expect(unwrapAudit(wrapFor({ tags: reset() }), guardianSk, NOW)).toBeNull();
  });

  it("an emergency unlock still comes through unwrapDeviceAudit as one", () => {
    const a = unwrapDeviceAudit(wrapFor(), guardianSk, NOW);
    expect(a?.op).toBe("unlock.breakglass");
  });

  it("reads a missing or malformed id as null, and falls back on the wrap's time", () => {
    const w = wrapFor({ tags: [["t", "charter-device"], ["outcome", "failed"], ["op", "screenlock.reset"], ["id", "nope"]] });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
      op: "screenlock.reset",
      reset: { machine: machinePk, id: null, result: "failed", at: NOW },
    });
  });

  it("never carries a PIN: the parsed audit has no such field", () => {
    const a = unwrapDeviceAudit(wrapFor({ tags: reset([["newPin", "246813"]]) }), guardianSk, NOW);
    expect(JSON.stringify(a)).not.toContain("246813");
  });

  // The outcomes the ward emits (core/crates/charter-proto ScreenLockResetResult).
  it.each(["applied", "failed", "not_ready", "notice_failed", "rejected_time", "superseded"])(
    "accepts the ward's outcome %s",
    (outcome) => {
      const w = wrapFor({ tags: reset([["at", String(NOW - 5)]], outcome) });
      expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
        op: "screenlock.reset",
        reset: { machine: machinePk, id: ID, result: outcome, at: NOW - 5 },
      });
    },
  );

  it("ignores an unknown outcome gracefully: still a screen-lock audit, never a throw, never an unlock", () => {
    for (const outcome of ["brand_new_outcome", "APPLIED", "applied ", "override"]) {
      const w = wrapFor({ tags: reset([], outcome) });
      expect(() => unwrapDeviceAudit(w, guardianSk, NOW)).not.toThrow();
      expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
        op: "screenlock.reset",
        reset: { machine: machinePk, id: ID, result: outcome, at: NOW },
      });
      expect(unwrapAudit(w, guardianSk, NOW)).toBeNull();
    }
  });

  it("reads a missing or empty outcome as null", () => {
    const noOutcome = wrapFor({ tags: [["t", "charter-device"], ["op", "screenlock.reset"], ["id", ID]] });
    expect(unwrapDeviceAudit(noOutcome, guardianSk, NOW)).toMatchObject({ reset: { result: null, id: ID } });
    const empty = wrapFor({ tags: reset([], "") });
    expect(unwrapDeviceAudit(empty, guardianSk, NOW)).toMatchObject({ reset: { result: null } });
  });

  it("rejects a forged sender and an unknown op", () => {
    expect(unwrapDeviceAudit(wrapFor({ tags: reset(), sealSk: generateSecretKey() }), guardianSk, NOW)).toBeNull();
    expect(unwrapDeviceAudit(wrapFor({ tags: [["t", "charter-device"], ["outcome", "applied"], ["op", "something.else"]] }), guardianSk, NOW)).toBeNull();
  });
});

describe("emergency call audit (op call.emergency)", () => {
  const day = 24 * 60 * 60;

  it("is accepted, with the press time, and keyed op-prefixed on the at tag", () => {
    const w = wrapFor({ tags: call([["at", String(NOW - 30)]]) });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
      op: "call.emergency",
      call: { machine: machinePk, at: NOW - 30, atStamped: true, key: `emergencycall:${machinePk}:${NOW - 30}` },
    });
  });

  it("is not an emergency unlock", () => {
    expect(unwrapAudit(wrapFor({ tags: call([["at", String(NOW - 30)]]) }), guardianSk, NOW)).toBeNull();
  });

  it("keys on the wrap, and dates by created_at, when there is no at tag", () => {
    const w = wrapFor({ createdAt: NOW - 90, tags: call() });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
      op: "call.emergency",
      call: { machine: machinePk, at: NOW - 90, atStamped: false, key: `emergencycall:wrap:${w.id}` },
    });
  });

  it("never collides with a break-glass press in the same second", () => {
    const t = String(NOW - 60);
    const unlock = unwrapDeviceAudit(wrapFor({ tags: override([["at", t]]) }), guardianSk, NOW);
    const emergency = unwrapDeviceAudit(wrapFor({ tags: call([["at", t]]) }), guardianSk, NOW);
    expect(unlock?.op === "unlock.breakglass" && unlock.override.key).toBe(`${machinePk}:${t}`);
    expect(emergency?.op === "call.emergency" && emergency.call.key).toBe(`emergencycall:${machinePk}:${t}`);
  });

  it("folds a rebuilt wrap (new id, same press) into one key", () => {
    const first = wrapFor({ tags: call([["at", String(NOW - 600)]]) });
    const rebuilt = wrapFor({ tags: call([["at", String(NOW - 600)]]), createdAt: NOW + 3600 });
    const a = unwrapDeviceAudit(first, guardianSk, NOW);
    const b = unwrapDeviceAudit(rebuilt, guardianSk, NOW + 3600);
    expect(first.id).not.toBe(rebuilt.id);
    expect(a?.op === "call.emergency" && a.call.key).toBe(b?.op === "call.emergency" && b.call.key);
    expect(a?.op === "call.emergency" && a.call.at).toBe(NOW - 600);
  });

  it("uses an at up to 30 days old, and not beyond, but still keys on it", () => {
    const ok = unwrapDeviceAudit(wrapFor({ tags: call([["at", String(NOW - 29 * day)]]) }), guardianSk, NOW);
    expect(ok).toMatchObject({ call: { at: NOW - 29 * day } });
    const old = unwrapDeviceAudit(wrapFor({ tags: call([["at", String(NOW - 31 * day)]]) }), guardianSk, NOW);
    expect(old).toMatchObject({ call: { at: NOW, key: `emergencycall:${machinePk}:${NOW - 31 * day}` } });
  });

  it("clamps a future at to the wrap's own time", () => {
    const w = wrapFor({ tags: call([["at", String(NOW + 1)]]) });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toMatchObject({
      call: { at: NOW, key: `emergencycall:${machinePk}:${NOW + 1}` },
    });
  });

  it.each(["-5", "1e9", " 12", "abc", ""])("ignores an unparseable at tag %j", (bad) => {
    const w = wrapFor({ tags: call([["at", bad]]) });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toMatchObject({
      call: { at: NOW, key: `emergencycall:wrap:${w.id}` },
    });
  });

  it("rejects a bad seal, a forged author, and a wrap outside the jitter window", () => {
    const t = call([["at", String(NOW - 5)]]);
    expect(unwrapDeviceAudit(wrapFor({ tags: t, sealSk: generateSecretKey() }), guardianSk, NOW)).toBeNull();
    expect(unwrapDeviceAudit(wrapFor({ tags: t, rumorPk: getPublicKey(generateSecretKey()) }), guardianSk, NOW)).toBeNull();
    expect(unwrapDeviceAudit(wrapFor({ tags: t, wrapAt: NOW - MAX_WRAP_JITTER_SECS - 1 }), guardianSk, NOW)).toBeNull();
  });

  // P3: the guardian is told on every use of the button, so the op alone decides.
  it("shows the emergency-call button whatever outcome the sender stamped on it, or none", () => {
    for (const outcome of ["enacted", "applied", "failed", "OVERRIDE", "", "dialler.unavailable"]) {
      const tags = [
        ["t", "charter-device"],
        ["outcome", outcome],
        ["op", "call.emergency"],
        ["at", String(NOW - 5)],
      ];
      expect(unwrapDeviceAudit(wrapFor({ tags }), guardianSk, NOW)).toMatchObject({
        op: "call.emergency",
        call: { machine: machinePk, at: NOW - 5, key: `emergencycall:${machinePk}:${NOW - 5}` },
      });
    }
    expect(unwrapDeviceAudit(wrapFor({ tags: [["op", "call.emergency"]] }), guardianSk, NOW)).toMatchObject({
      op: "call.emergency",
    });
  });

  it("folds a press stamped with another outcome into the same key as a conforming one", () => {
    const a = unwrapDeviceAudit(wrapFor({ tags: call([["at", String(NOW - 5)]]) }), guardianSk, NOW);
    const b = unwrapDeviceAudit(
      wrapFor({ tags: [["op", "call.emergency"], ["outcome", "failed"], ["at", String(NOW - 5)]] }),
      guardianSk,
      NOW,
    );
    expect(a?.op === "call.emergency" && a.call.key).toBe(b?.op === "call.emergency" && b.call.key);
  });

  it("carries nothing but machine, at and key", () => {
    const w = wrapFor({ tags: call([["at", String(NOW - 5)], ["number", "112233"], ["durationSecs", "9"]]) });
    const a = unwrapDeviceAudit(w, guardianSk, NOW);
    expect(a?.op === "call.emergency" && Object.keys(a.call).sort()).toEqual(["at", "atStamped", "key", "machine"]);
    expect(JSON.stringify(a)).not.toContain("112233");
  });
});

describe("an unknown op under outcome=override", () => {
  const ctl = (op: string, extra: string[][] = []): string[][] => [
    ["t", "charter-device"],
    ["outcome", "override"],
    ["op", op],
    ...extra,
  ];

  it("is a generic emergency control, keyed on its op, machine and press time", () => {
    const w = wrapFor({ tags: ctl("alarm.silence", [["at", String(NOW - 20)]]) });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
      op: "override.other",
      control: { machine: machinePk, at: NOW - 20, atStamped: true, key: `override:alarm.silence:${machinePk}:${NOW - 20}` },
    });
  });

  it("keys on the wrap and dates by created_at without an at tag", () => {
    const w = wrapFor({ createdAt: NOW - 90, tags: ctl("alarm.silence") });
    expect(unwrapDeviceAudit(w, guardianSk, NOW)).toEqual({
      op: "override.other",
      control: { machine: machinePk, at: NOW - 90, atStamped: false, key: `override:alarm.silence:wrap:${w.id}` },
    });
  });

  it("marks the press time as the ward's own only when its at tag is fit to date by", () => {
    const stamped = (tags: string[][], createdAt = NOW) => {
      const a = unwrapDeviceAudit(wrapFor({ createdAt, tags }), guardianSk, NOW);
      return a?.op === "override.other" && a.control.atStamped;
    };
    expect(stamped(ctl("alarm.silence", [["at", String(NOW - 20)]]))).toBe(true);
    // No tag, a tag after its own wrap, or one absurdly old: the wrap's time stands in.
    expect(stamped(ctl("alarm.silence"))).toBe(false);
    expect(stamped(ctl("alarm.silence", [["at", String(NOW + 50)]]))).toBe(false);
    expect(stamped(ctl("alarm.silence", [["at", "5"]]))).toBe(false);
  });

  it("two different ops in the same second are two keys, and neither is a break-glass or call key", () => {
    const t = String(NOW - 5);
    const a = unwrapDeviceAudit(wrapFor({ tags: ctl("x.one", [["at", t]]) }), guardianSk, NOW);
    const b = unwrapDeviceAudit(wrapFor({ tags: ctl("x.two", [["at", t]]) }), guardianSk, NOW);
    const keyOf = (v: typeof a) => (v?.op === "override.other" ? v.control.key : "");
    expect(keyOf(a)).not.toBe(keyOf(b));
    expect(keyOf(a)).not.toBe(`${machinePk}:${t}`);
    expect(keyOf(a)).not.toBe(`emergencycall:${machinePk}:${t}`);
  });

  it("folds a rebuilt wrap (new id, same press) into one key", () => {
    const tags = ctl("alarm.silence", [["at", String(NOW - 600)]]);
    const a = unwrapDeviceAudit(wrapFor({ tags }), guardianSk, NOW);
    const b = unwrapDeviceAudit(wrapFor({ tags, createdAt: NOW + 3600 }), guardianSk, NOW + 3600);
    expect(a?.op === "override.other" && a.control.key).toBe(b?.op === "override.other" && b.control.key);
  });

  it("cuts a very long op in the key, and rejects a bad seal or any other outcome", () => {
    const long = "x".repeat(500);
    const a = unwrapDeviceAudit(wrapFor({ tags: ctl(long, [["at", String(NOW - 1)]]) }), guardianSk, NOW);
    expect(a?.op === "override.other" && a.control.key.length).toBeLessThan(200);
    expect(unwrapDeviceAudit(wrapFor({ tags: ctl("a.b"), sealSk: generateSecretKey() }), guardianSk, NOW)).toBeNull();
    expect(
      unwrapDeviceAudit(wrapFor({ tags: [["outcome", "enacted"], ["op", "a.b"]] }), guardianSk, NOW),
    ).toBeNull();
  });

  // P2: the carrier alerts on an override with no op as an emergency unlock,
  // so the console shows it as one, with less detail, rather than dropping it.
  it("shows an empty or missing op as an emergency unlock with less detail", () => {
    for (const tags of [ctl("", [["at", String(NOW - 5)]]), [["outcome", "override"], ["at", String(NOW - 5)]]]) {
      const got = unwrapDeviceAudit(wrapFor({ tags }), guardianSk, NOW);
      expect(got).toMatchObject({
        op: "unlock.breakglass",
        override: { machine: machinePk, scope: null, durationSecs: null, at: NOW - 5, key: `${machinePk}:${NOW - 5}` },
      });
    }
  });

  it("still shows nothing for an empty or missing op under any other outcome", () => {
    expect(unwrapDeviceAudit(wrapFor({ tags: [["outcome", "enacted"], ["op", ""]] }), guardianSk, NOW)).toBeNull();
    expect(unwrapDeviceAudit(wrapFor({ tags: [["outcome", "enacted"]] }), guardianSk, NOW)).toBeNull();
    expect(unwrapDeviceAudit(wrapFor({ tags: [["t", "charter-device"]] }), guardianSk, NOW)).toBeNull();
  });

  it("does not change the known ops", () => {
    expect(unwrapDeviceAudit(wrapFor({ tags: ctl("unlock.breakglass") }), guardianSk, NOW)?.op).toBe("unlock.breakglass");
    expect(unwrapDeviceAudit(wrapFor({ tags: ctl("call.emergency") }), guardianSk, NOW)?.op).toBe("call.emergency");
    expect(unwrapDeviceAudit(wrapFor({ tags: ctl("screenlock.reset") }), guardianSk, NOW)?.op).toBe("screenlock.reset");
  });
});
