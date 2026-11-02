// The guardian's read side of the ward's emergency unlock (kind 31000).
import { describe, expect, it } from "vitest";
import { generateSecretKey, getPublicKey, type NostrEvent } from "nostr-tools";
import { unwrapAudit } from "./audit";
import { CHARTER_DEVICE_REQUEST, MAX_WRAP_JITTER_SECS } from "./request";
import { CHARTER_DEVICE_STATUS } from "./status";
import { overrideTags as override, overrideWrap, type WrapOpts } from "../test/overrideWrap";

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
