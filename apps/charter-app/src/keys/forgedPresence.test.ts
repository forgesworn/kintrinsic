// Regression (batch 3 re-review, N-H1): a forged presence code must not make
// an attacker a guardian in the joining parent's app.
//
// The attack: parent B scans a code naming the attacker's machine EVIL with
// `d = kintrinsic/guardian/<R_B>` (R_B is public). Before the fix, EVIL's
// STATUS (signed only by EVIL, so it can claim any roots) made the attacker's
// root R_X a trusted roster source: B's roster went to the attacker, the
// attacker's roster was adopted with its device shown as confirmed, and a
// real child was renamed and given attacker devices that then received its
// rules and the guardian PIN.
//
// The fix (OQ "Anchoring a joining parent's trust"): B trusts only its own
// root and roots it anchored (the inviting parent's scanned guardian code, or
// a root it invited). A presence-learned machine gets the 31121 confirmation
// and nothing else until an anchored roster names it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { finalizeEvent, generateSecretKey, getPublicKey, nip19, nip44, type NostrEvent } from "nostr-tools";
import { GuardianKeys, unwrapToWorkingKey } from "./guardianKeys";
import { emptyGuardianKeys, type GuardianKeysState } from "./state";
import { CHARTER_FAMILY_ROSTER, CHARTER_PRESENCE_CONFIRM, type SignedEvent } from "./sdk";
import type { StatusGuardians } from "./statusGuardians";
import type { GuardianOps } from "../wire/giftwrap";
import type { Child, Device } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { adoptRoster, applyDeviceKeysView, keysChildren } from "../store/guardianKeysWiring";
import { admitStatus } from "../store/statusAdmission";
import { ingestWrap, type WrapIngestContext } from "../store/ingestWrap";
import { familyWards, androidWardsWithoutPin } from "../screens/guardiansView";
import { resolveChildTarget } from "../signer/resolveChild";

const T = 1_800_000_000;
const SUBJECT = "ab".repeat(32);

function opsOf(sk: Uint8Array): GuardianOps {
  return {
    pubkey: getPublicKey(sk),
    signEvent: async (t) => finalizeEvent(t, sk),
    nip44Encrypt: async (to, pt) => nip44.encrypt(pt, nip44.getConversationKey(sk, to)),
  };
}

const rASk = generateSecretKey();
const wASk = generateSecretKey();
const rBSk = generateSecretKey();
const wBSk = generateSecretKey();
const rXSk = generateSecretKey();
const wXSk = generateSecretKey();
const R_A = getPublicKey(rASk);
const W_A = getPublicKey(wASk);
const R_B = getPublicKey(rBSk);
const W_B = getPublicKey(wBSk);
const R_X = getPublicKey(rXSk);
const W_X = getPublicKey(wXSk);
const wOldSk = generateSecretKey(); // A's withdrawn (lost, stolen) working key
const W_OLD = getPublicKey(wOldSk);
const rA2Sk = generateSecretKey(); // A's new identity after a move
const R_A2 = getPublicKey(rA2Sk);
const PHONE = getPublicKey(generateSecretKey());
const LAPTOP = getPublicKey(generateSecretKey());
const evilSk = generateSecretKey();
const EVIL = getPublicKey(evilSk);
const EVIL2 = getPublicKey(generateSecretKey());

interface Install {
  gk: GuardianKeys;
  keys: () => GuardianKeysState;
  children: () => Child[];
  setChildren: (c: Child[]) => void;
  published: { relays: string[]; wrap: NostrEvent }[];
}

function install(wSk: Uint8Array, rootSk: Uint8Array, children: Child[], now = () => T): Install {
  let keys = emptyGuardianKeys();
  let kids = children;
  const published: Install["published"] = [];
  const gk = new GuardianKeys({
    read: () => ({ keys, children: keysChildren(kids, ["wss://d.example"]) }),
    write: (f) => {
      keys = f(keys);
    },
    workingKey: () => wSk,
    root: () => opsOf(rootSk),
    publish: async (relays, wrap) => {
      published.push({ relays, wrap });
    },
    now,
    defaultRelays: ["wss://d.example"],
  });
  return { gk, keys: () => keys, children: () => kids, setChildren: (c) => (kids = c), published };
}

const g = (roots: { root: string; keys: string[] }[]): StatusGuardians => ({
  roots,
  pending: [],
  tombstones: 0,
  relays: ["wss://w1.example"],
});

function ward(machine: string, platform: Device["platform"], label: string, guardians: StatusGuardians, pinned: string): Device {
  return {
    id: `dev-${label}`,
    label,
    platform,
    pairing: "paired",
    devicePubkey: machine,
    pinnedKey: pinned,
    keysViewAt: T - 100,
    appVersionCode: platform === "android" ? 49 : 713,
    guardians,
  };
}

const status = (machine: string, ts: number, guardians: StatusGuardians): DeviceStatus =>
  ({ v: 1, machine, ts, appVersionCode: 49, guardians }) as unknown as DeviceStatus;

const code = (machine: string, d: string, s: string) =>
  `charter://presence?m=${machine}&t=${"0".repeat(32)}&d=${encodeURIComponent(d)}&s=${s}`;

let n = 0;
const mint = () => ({ mintChildId: () => `c${++n}`, mintDeviceId: () => `d${++n}`, mintPolicyId: () => `p${++n}` });

/** The store's roster intake for install B, as `wrapContext` wires it. */
function intakeFor(b: Install): WrapIngestContext & { outcomes: string[] } {
  const outcomes: string[] = [];
  return {
    outcomes,
    guardianSk: wBSk,
    handledWraps: new Set(),
    seenReqIds: new Set(),
    hasRequest: () => false,
    ingestStatus: () => {},
    ingestRequest: () => false,
    ingestRoster: (inbound) => {
      const r = b.gk.receiveRoster(inbound);
      outcomes.push(r.ok ? "ok" : r.reason);
      if (!r.ok) return r.reason;
      const k = b.keys();
      b.setChildren(adoptRoster(b.children(), k.roster, r.updated, { presenceWards: k.presenceWards, ...mint() }));
      b.gk.prunePresenceWards();
      return "ok";
    },
  };
}

const wrapsTo = (p: { wrap: NostrEvent }[], to: string) =>
  p.filter((x) => x.wrap.tags.some((t) => t[0] === "p" && t[1] === to)).map((x) => x.wrap);

/** A, with Sam (phone and laptop), invites B; B anchors A, confirms at the phone. */
async function joined(opts: { rosterBeforeStatus?: boolean } = {}) {
  const onlyA = g([{ root: R_A, keys: [W_A] }]);
  const both = g([
    { root: R_A, keys: [W_A] },
    { root: R_B, keys: [W_B] },
  ]);
  const A = install(wASk, rASk, [
    {
      id: "a1",
      name: "Sam",
      color: "#111111",
      dependantPubkey: null,
      rosterId: SUBJECT,
      policies: [],
      devices: [ward(PHONE, "android", "Sam's phone", onlyA, R_A), ward(LAPTOP, "linux", "Sam's laptop", onlyA, R_A)],
    },
  ]);
  await A.gk.approveThisInstall({ via: "signet", label: "A" });
  const invite = await A.gk.inviteCoGuardian(R_B);
  const B = install(wBSk, rBSk, []);
  await B.gk.approveThisInstall({ via: "signet", label: "B", expectRoot: R_B });
  // B scans A's guardian code (Join a family), then confirms in person.
  B.gk.anchorRoot(nip19.npubEncode(R_A));
  await B.gk.confirmPresence(code(PHONE, `kintrinsic/guardian/${R_B}`, invite.event.id));
  const ctx = intakeFor(B);
  if (!opts.rosterBeforeStatus) B.gk.notePresenceStatus(status(PHONE, T + 10, both));
  A.setChildren(applyDeviceKeysView(A.children(), status(PHONE, T + 10, both)));
  const sent = await A.gk.sendRoster();
  expect(sent.sent).toEqual([W_B]);
  expect(ingestWrap(wrapsTo(A.published, W_B).at(-1)!, ctx).kind).toBe("roster");
  if (opts.rosterBeforeStatus) {
    // No ward B vouches for has reported yet: the roster is held, not adopted.
    expect(ctx.outcomes.at(-1)).toBe("held");
    expect(B.children()).toEqual([]);
    B.gk.notePresenceStatus(status(PHONE, T + 10, both));
    const updated = B.gk.releaseHeldRosters();
    expect(updated).toEqual([SUBJECT]);
    const k = B.keys();
    B.setChildren(adoptRoster(B.children(), k.roster, updated, { presenceWards: k.presenceWards, ...mint() }));
    B.gk.prunePresenceWards();
    expect(B.keys().heldRosters).toEqual({});
  } else {
    expect(ctx.outcomes.at(-1)).toBe("ok");
  }
  return { A, B, ctx, both };
}

describe("a forged presence code (batch 3 re-review N-H1)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T * 1000);
  });
  afterEach(() => vi.useRealTimers());

  it("gets the confirmation and nothing else: no roster, no adoption, no rules, no PIN", async () => {
    const { B, ctx } = await joined();
    expect(B.children().map((c) => c.name)).toEqual(["Sam"]);
    const samId = B.children()[0].id;

    // The attacker hands B a code naming EVIL with d = guardian/<R_B> (R_B is public).
    const invD = `kintrinsic/guardian/${R_B}`;
    const before = B.published.length;
    const rep = await B.gk.confirmPresence(code(EVIL, invD, "2".repeat(64)));
    // The confirmation itself must reach the machine the code names; it is the only thing that does.
    expect(rep.sent).toEqual([EVIL]);
    expect(B.published.length).toBe(before + 1);
    const confirmation = unwrapToWorkingKey(B.published.at(-1)!.wrap, wXSk);
    expect(confirmation).toBeNull(); // addressed to EVIL, not readable by the attacker's W
    expect(B.gk.isPresenceWard(EVIL)).toBe(true);

    // EVIL's STATUS is admitted for display only. It claims the attacker's
    // root, and even the attacker's W under the anchored root R_A.
    const evilView = g([
      { root: R_X, keys: [W_X] },
      { root: R_A, keys: [W_X] },
      { root: R_B, keys: [W_B] },
    ]);
    expect(admitStatus(status(EVIL, T + 20, evilView), B.children(), new Set(), (m) => B.gk.isPresenceWard(m))).toBe("presence");
    B.gk.notePresenceStatus(status(EVIL, T + 20, evilView));
    expect(B.gk.reportedRoots().map((r) => r.root)).not.toContain(R_X);
    expect(B.gk.reportedRoots().flatMap((r) => r.keys)).not.toContain(W_X);
    expect(B.gk.trustedRoots()).toEqual([R_A, R_B].sort());

    // B's roster (Sam, the phone, the laptop) never goes to W_X.
    const early = await B.gk.sendRoster();
    expect(early.sent).not.toContain(W_X);
    expect(wrapsTo(B.published, W_X)).toEqual([]);

    // The attacker's own install sends a roster: a fake child holding EVIL.
    const X = install(wXSk, rXSk, [
      {
        id: "x1",
        name: "Fake kid",
        color: "#ff0000",
        dependantPubkey: null,
        rosterId: "cd".repeat(32),
        policies: [],
        devices: [ward(EVIL, "android", "Fake phone", g([{ root: R_X, keys: [W_X] }, { root: R_B, keys: [W_B] }]), R_X)],
      },
    ]);
    await X.gk.approveThisInstall({ via: "signet", label: "X" });
    X.gk.anchorRoot(R_B);
    await X.gk.sendRoster();
    const xWrap = wrapsTo(X.published, W_B).at(-1)!;
    expect(ingestWrap(xWrap, ctx).kind).toBe("roster");
    expect(ctx.outcomes.at(-1)).toBe("root_not_trusted");
    // Refused for good: the wrap is handled, not retried every poll (N-L2).
    expect(ctx.handledWraps.has(xWrap.id)).toBe(true);
    expect(ingestWrap(xWrap, ctx).kind).toBe("handled");
    expect(B.children().map((c) => c.name)).toEqual(["Sam"]);
    expect(B.gk.unconfirmedRosterDevices().map((d) => d.machine)).not.toContain(EVIL);

    // With the subject (a guess, or a leak elsewhere), the attacker rewrites Sam:
    // renamed, the unheard laptop dropped, EVIL2 added. Refused.
    vi.setSystemTime((T + 200) * 1000);
    const X2 = install(
      wXSk,
      rXSk,
      [
        {
          id: "x2",
          name: "Renamed",
          color: "#000000",
          dependantPubkey: null,
          rosterId: SUBJECT,
          policies: [],
          devices: [
            ward(PHONE, "android", "Sam's phone", g([{ root: R_A, keys: [W_A] }]), R_A),
            ward(EVIL2, "android", "Sam's tablet", g([{ root: R_X, keys: [W_X] }, { root: R_B, keys: [W_B] }]), R_X),
          ],
        },
      ],
      () => T + 200,
    );
    await X2.gk.approveThisInstall({ via: "signet", label: "X" });
    X2.gk.anchorRoot(R_B);
    await X2.gk.sendRoster();
    expect(ingestWrap(wrapsTo(X2.published, W_B).at(-1)!, ctx).kind).toBe("roster");
    expect(ctx.outcomes.at(-1)).toBe("root_not_trusted");

    const sam = B.children().find((c) => c.rosterId === SUBJECT)!;
    expect(sam.name).toBe("Sam");
    const machines = sam.devices.map((d) => d.devicePubkey);
    expect(machines).toContain(LAPTOP);
    expect(machines).not.toContain(EVIL);
    expect(machines).not.toContain(EVIL2);

    // No rules and no PIN reach EVIL or EVIL2.
    const target = resolveChildTarget(B.children(), samId, [])!.devices.map((d) => d.pubkey);
    expect(target).not.toContain(EVIL);
    expect(target).not.toContain(EVIL2);
    const wards = androidWardsWithoutPin(familyWards(B.children()), B.keys()).map((w) => w.device.devicePubkey);
    expect(wards).not.toContain(EVIL);
    expect(wards).not.toContain(EVIL2);
    const pin = await B.gk.setGuardianPin("123456");
    expect([...pin.sent, ...pin.held, ...pin.failed.map((f) => f.to)]).not.toContain(EVIL);
    expect([...pin.sent, ...pin.held, ...pin.failed.map((f) => f.to)]).not.toContain(EVIL2);
    expect(await B.gk.syncApprovals({ force: true })).not.toContain(EVIL);
    expect(admitStatus(status(EVIL2, T + 210, g([{ root: R_X, keys: [W_X] }])), B.children(), new Set(), (m) => B.gk.isPresenceWard(m))).toBe(
      "stranger",
    );
    // Across everything above, EVIL got exactly one wrap from B: the 31121 confirmation.
    expect(wrapsTo(B.published, EVIL).map((w) => unwrapToWorkingKey(w, evilSk)?.inner.kind)).toEqual([CHARTER_PRESENCE_CONFIRM]);
    expect(wrapsTo(B.published, EVIL2)).toEqual([]);
  });

  it("a roster whose approval names another key, or that copies an anchored root's approval, is refused", async () => {
    const { A, B } = await joined();
    // The attacker copies A's approval into a roster it signs with W_X.
    const fromA = unwrapToWorkingKey(wrapsTo(A.published, W_B).at(-1)!, wBSk)!;
    const content = JSON.parse(fromA.inner.content) as { approval: SignedEvent };
    const forged = finalizeEvent(
      {
        kind: CHARTER_FAMILY_ROSTER,
        created_at: T,
        tags: [],
        content: JSON.stringify({
          v: 1,
          children: [{ subject: SUBJECT, issuedAt: T + 100, name: "Renamed", colour: "#000000", devices: [] }],
          approval: content.approval,
        }),
      },
      wXSk,
    ) as SignedEvent;
    expect(B.gk.receiveRoster({ sealAuthor: W_X, inner: forged })).toEqual({ ok: false, reason: "bad_approval" });
    expect(B.children()[0].name).toBe("Sam");
  });

  it("a presence-only machine gets nothing but the confirmation before any roster", async () => {
    const B = install(wBSk, rBSk, []);
    await B.gk.approveThisInstall({ via: "signet", label: "B", expectRoot: R_B });
    // Without the inviting parent's guardian code, B can't confirm a joining invitation at all.
    await expect(B.gk.confirmPresence(code(EVIL, `kintrinsic/guardian/${R_B}`, "2".repeat(64)))).rejects.toThrow(
      /guardian code of the parent who invited you/,
    );
    B.gk.anchorRoot(nip19.npubEncode(R_A));
    await B.gk.confirmPresence(code(EVIL, `kintrinsic/guardian/${R_B}`, "2".repeat(64)));
    B.gk.notePresenceStatus(status(EVIL, T + 5, g([{ root: R_A, keys: [W_X] }, { root: R_X, keys: [W_X] }])));
    const inner = unwrapToWorkingKey(B.published[0].wrap, wBSk);
    expect(inner).toBeNull();
    expect(B.published).toHaveLength(1);
    expect((await B.gk.sendRoster()).sent).toEqual([]);
    await expect(B.gk.setGuardianPin("123456")).rejects.toThrow(/No Android device/);
    expect(await B.gk.syncApprovals({ force: true })).toEqual([]);
    await expect(B.gk.leaveFamily()).rejects.toThrow(/hasn't shared the family/);
    expect(B.published).toHaveLength(1);
    // The one wrap is the 31121 confirmation, to EVIL.
    expect(unwrapToWorkingKey(B.published[0].wrap, evilSk)?.inner.kind).toBe(CHARTER_PRESENCE_CONFIRM);
  });
});

describe("a parent who has left or been removed", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T * 1000);
  });
  afterEach(() => vi.useRealTimers());

  it("stays anchored but can't send a roster once no ward reports their root", async () => {
    const { A, B } = await joined();
    // While the wards report R_B, B's roster reaches A and is taken in.
    await B.gk.sendRoster();
    const first = unwrapToWorkingKey(wrapsTo(B.published, W_A).at(-1)!, wASk)!;
    expect(A.gk.receiveRoster(first)).toMatchObject({ ok: true });
    // B is removed: the phone now reports only A. B's root is still anchored at A.
    A.setChildren(applyDeviceKeysView(A.children(), status(PHONE, T + 500, g([{ root: R_A, keys: [W_A] }]))));
    expect(A.gk.trustedRoots()).toContain(R_B);
    B.setChildren(B.children().map((c) => ({ ...c, name: "Renamed" })));
    await B.gk.sendRoster();
    const later = unwrapToWorkingKey(wrapsTo(B.published, W_A).at(-1)!, wASk)!;
    expect(A.gk.receiveRoster(later)).toEqual({ ok: false, reason: "root_not_reported" });
    expect(A.children()[0].name).toBe("Sam");
  });
});

describe("the honest join still works end to end", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T * 1000);
  });
  afterEach(() => vi.useRealTimers());

  it("A invites, B anchors A, B confirms, A's roster arrives (before the ward's STATUS), B sees the family and can set a rule", async () => {
    const { A, B, both } = await joined({ rosterBeforeStatus: true });
    const sam = B.children().find((c) => c.rosterId === SUBJECT)!;
    expect(sam).toMatchObject({ name: "Sam", color: "#111111" });
    expect(sam.devices.map((d) => [d.devicePubkey, d.fromRoster])).toEqual(
      expect.arrayContaining([
        [PHONE, true],
        [LAPTOP, true],
      ]),
    );
    expect(sam.devices).toHaveLength(2);
    // The phone's STATUS arrives after: it is now a family device.
    const s = status(PHONE, T + 20, both);
    expect(admitStatus(s, B.children(), new Set(), (m) => B.gk.isPresenceWard(m))).toBe("claimed");
    B.setChildren(applyDeviceKeysView(B.children(), s));
    expect(B.keys().presenceWards).toEqual({});
    // B can set a rule: the phone is a delivery target and W_B signs for it.
    expect(resolveChildTarget(B.children(), sam.id, [])!.devices.map((d) => d.pubkey)).toContain(PHONE);
    expect(B.gk.keyFor(PHONE)).toBe("working");
    // And B's own roster goes back to A, whose root B anchored.
    const back = await B.gk.sendRoster();
    expect(back.sent).toEqual([W_A]);
    const opened = unwrapToWorkingKey(wrapsTo(B.published, W_A).at(-1)!, wASk)!;
    expect(A.gk.receiveRoster(opened)).toMatchObject({ ok: true });
  });
});

describe("a withdrawn working key of a trusted root (re-review N2-M1, probe P3)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T * 1000);
  });
  afterEach(() => vi.useRealTimers());

  it("can't plant a device in the join window, or ever have a roster accepted", async () => {
    const B = install(wBSk, rBSk, []);
    await B.gk.approveThisInstall({ via: "signet", label: "B", expectRoot: R_B });
    B.gk.anchorRoot(R_A);
    // The thief holds A's withdrawn W_OLD and its never-expiring approval by R_A.
    const evilView = g([
      { root: R_A, keys: [W_A, W_OLD] },
      { root: R_B, keys: [W_B] },
    ]);
    const thief = install(wOldSk, rASk, [
      { id: "t1", name: "Fake", color: "#ff0000", dependantPubkey: null, rosterId: "cd".repeat(32), policies: [], devices: [ward(EVIL, "android", "Fake phone", evilView, R_A)] },
    ]);
    await thief.gk.approveThisInstall({ via: "signet", label: "lost" });
    thief.gk.anchorRoot(R_B);
    await thief.gk.sendRoster();
    const ctx = intakeFor(B);
    // Before any ward B vouches for has reported: held, not adopted.
    expect(ingestWrap(wrapsTo(thief.published, W_B).at(-1)!, ctx).kind).toBe("roster");
    expect(ctx.outcomes.at(-1)).toBe("held");
    expect(B.children()).toEqual([]);
    expect(admitStatus(status(EVIL, T + 30, evilView), B.children(), new Set(), (m) => B.gk.isPresenceWard(m))).toBe("stranger");

    // B confirms in person at the real phone, which lists only A's live key.
    const both = g([
      { root: R_A, keys: [W_A] },
      { root: R_B, keys: [W_B] },
    ]);
    await B.gk.confirmPresence(code(PHONE, `kintrinsic/guardian/${R_B}`, "3".repeat(64)));
    B.gk.notePresenceStatus(status(PHONE, T + 40, both));
    // The held roster is re-checked now and dropped: no ward B vouches for lists W_OLD.
    expect(B.gk.releaseHeldRosters()).toEqual([]);
    expect(B.keys().heldRosters).toEqual({});
    expect(B.children()).toEqual([]);

    // A's real roster lands.
    const A = install(wASk, rASk, [
      {
        id: "a1",
        name: "Sam",
        color: "#111111",
        dependantPubkey: null,
        rosterId: SUBJECT,
        policies: [],
        devices: [ward(PHONE, "android", "Sam's phone", both, R_A), ward(LAPTOP, "linux", "Sam's laptop", both, R_A)],
      },
    ]);
    await A.gk.approveThisInstall({ via: "signet", label: "A" });
    A.gk.anchorRoot(R_B);
    await A.gk.sendRoster();
    expect(ingestWrap(wrapsTo(A.published, W_B).at(-1)!, ctx).kind).toBe("roster");
    expect(ctx.outcomes.at(-1)).toBe("ok");
    expect(B.children().map((c) => c.name)).toEqual(["Sam"]);

    // B's roster never goes to the withdrawn key.
    const out = await B.gk.sendRoster();
    expect(out.sent).toEqual([W_A]);
    expect(wrapsTo(B.published, W_OLD)).toEqual([]);

    // A later roster from the thief, with the leaked subject, is refused for good.
    vi.setSystemTime((T + 200) * 1000);
    const thief2 = install(
      wOldSk,
      rASk,
      [
        {
          id: "t2",
          name: "Renamed",
          color: "#000000",
          dependantPubkey: null,
          rosterId: SUBJECT,
          policies: [],
          devices: [ward(PHONE, "android", "Sam's phone", evilView, R_A), ward(EVIL2, "android", "Sam's tablet", evilView, R_A)],
        },
      ],
      () => T + 200,
    );
    await thief2.gk.approveThisInstall({ via: "signet", label: "lost" });
    thief2.gk.anchorRoot(R_B);
    await thief2.gk.sendRoster();
    expect(ingestWrap(wrapsTo(thief2.published, W_B).at(-1)!, ctx).kind).toBe("roster");
    expect(ctx.outcomes.at(-1)).toBe("key_not_reported");
    const sam = B.children().find((c) => c.rosterId === SUBJECT)!;
    expect(sam.name).toBe("Sam");
    expect(sam.devices.map((d) => d.devicePubkey)).not.toContain(EVIL2);
    expect(resolveChildTarget(B.children(), sam.id, [])!.devices.map((d) => d.pubkey)).not.toContain(EVIL2);
  });

  it("a device a roster introduced never vouches for a roster sender", async () => {
    const { B, ctx } = await joined();
    // The laptop came from A's roster and B never confirmed there. Even if it
    // reports W_OLD under R_A, it doesn't vouch for W_OLD.
    const laptopView = g([
      { root: R_A, keys: [W_A, W_OLD] },
      { root: R_B, keys: [W_B] },
    ]);
    B.setChildren(applyDeviceKeysView(B.children(), status(LAPTOP, T + 50, laptopView)));
    expect(B.gk.vouchedRoots().flatMap((r) => r.keys)).not.toContain(W_OLD);
    expect((await B.gk.sendRoster()).sent).not.toContain(W_OLD);
    const thief = install(wOldSk, rASk, [
      { id: "t1", name: "Renamed", color: "#000000", dependantPubkey: null, rosterId: SUBJECT, policies: [], devices: [] },
    ]);
    await thief.gk.approveThisInstall({ via: "signet", label: "lost" });
    thief.gk.anchorRoot(R_B);
    thief.setChildren(thief.children().map((c) => ({ ...c, devices: [ward(LAPTOP, "linux", "Sam's laptop", laptopView, R_A)] })));
    await thief.gk.sendRoster();
    expect(ingestWrap(wrapsTo(thief.published, W_B).at(-1)!, ctx).kind).toBe("roster");
    expect(ctx.outcomes.at(-1)).toBe("key_not_reported");
    expect(B.children()[0].name).toBe("Sam");
  });
});

describe("the inviting parent changes identity (re-review N2-L1, probe P4)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T * 1000);
  });
  afterEach(() => vi.useRealTimers());

  it("says the family list stopped, and resumes once their new guardian code is scanned", async () => {
    const { B } = await joined();
    expect(B.gk.movedAwayRoots()).toEqual([]);
    // A's move to R_A2 completes: the wards report W_A under R_A2.
    const moved = g([
      { root: R_A2, keys: [W_A] },
      { root: R_B, keys: [W_B] },
    ]);
    for (const m of [PHONE, LAPTOP]) B.setChildren(applyDeviceKeysView(B.children(), status(m, T + 50, moved)));
    const A2 = install(
      wASk,
      rA2Sk,
      [
        {
          id: "a1",
          name: "Sam (new name)",
          color: "#111111",
          dependantPubkey: null,
          rosterId: SUBJECT,
          policies: [],
          devices: [ward(PHONE, "android", "Sam's phone", moved, R_A2), ward(LAPTOP, "linux", "Sam's laptop", moved, R_A2)],
        },
      ],
      () => T + 60,
    );
    await A2.gk.approveThisInstall({ via: "signet", label: "A" });
    A2.gk.anchorRoot(R_B);
    await A2.gk.sendRoster();
    const fromA2 = unwrapToWorkingKey(wrapsTo(A2.published, W_B).at(-1)!, wBSk)!;
    // Fail closed: refused, and B's roster no longer goes to W_A...
    expect(B.gk.receiveRoster(fromA2)).toEqual({ ok: false, reason: "root_not_trusted" });
    expect((await B.gk.sendRoster()).sent).not.toContain(W_A);
    // ...but the app can say so.
    expect(B.gk.movedAwayRoots()).toEqual([R_A]);
    // B scans A's new guardian code: sharing resumes.
    B.gk.anchorRoot(nip19.npubEncode(R_A2));
    expect(B.gk.movedAwayRoots()).toEqual([]);
    vi.setSystemTime((T + 60) * 1000);
    expect(B.gk.receiveRoster(fromA2)).toMatchObject({ ok: true, updated: [SUBJECT] });
    expect((await B.gk.sendRoster()).sent).toContain(W_A);
  });
});
