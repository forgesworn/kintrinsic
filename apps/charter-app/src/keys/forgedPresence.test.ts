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
  expect(ctx.outcomes.at(-1)).toBe("ok");
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
    expect(sam.devices.map((d) => [d.devicePubkey, d.fromRoster])).toEqual([
      [PHONE, true],
      [LAPTOP, true],
    ]);
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
