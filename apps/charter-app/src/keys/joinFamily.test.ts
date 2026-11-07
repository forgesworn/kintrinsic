// A second parent joins a family, end to end across two guardian installs
// and a stand-in ward (contract, *Guardian roots and working keys (v1)*:
// co-guardian, presence confirmation, *Family roster*, rule 7):
//
//   install A (root R_A, working key W_A) has a child with two wards;
//   A invites R_B; install B approves W_B, scans A's guardian code (its
//   anchor) and confirms in person at the phone; the phone's STATUS to W_B
//   is admitted from the presence code alone, for display; A's roster
//   (trusted because R_A is anchored) becomes B's family; B can then sign
//   for the wards, object to a removal, leave and withdraw a phone of its
//   own.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { finalizeEvent, generateSecretKey, getPublicKey, nip19, nip44, type NostrEvent } from "nostr-tools";
import { GuardianKeys, unwrapToWorkingKey } from "./guardianKeys";
import { emptyGuardianKeys, type GuardianKeysState } from "./state";
import { parseStatement, type SignedEvent } from "./sdk";
import type { StatusGuardians } from "./statusGuardians";
import type { GuardianOps } from "../wire/giftwrap";
import type { Child, Device } from "../domain/types";
import type { DeviceStatus } from "../wire/status";
import { adoptRoster, applyDeviceKeysView, keysChildren } from "../store/guardianKeysWiring";
import { admitStatus } from "../store/statusAdmission";
import { ingestWrap, type WrapIngestContext } from "../store/ingestWrap";
import { pendingFor, removalsAgainstMe, familyWards, leaveGate } from "../screens/guardiansView";

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
const R_A = getPublicKey(rASk);
const W_A = getPublicKey(wASk);
const R_B = getPublicKey(rBSk);
const W_B = getPublicKey(wBSk);
const W_B2 = getPublicKey(generateSecretKey()); // B's other phone
const phoneSk = generateSecretKey();
const laptopSk = generateSecretKey();
const PHONE = getPublicKey(phoneSk);
const LAPTOP = getPublicKey(laptopSk);

interface Install {
  gk: GuardianKeys;
  keys: () => GuardianKeysState;
  children: () => Child[];
  setChildren: (c: Child[]) => void;
  published: { relays: string[]; wrap: NostrEvent }[];
}

function install(wSk: Uint8Array, rootSk: Uint8Array, children: Child[]): Install {
  let keys = emptyGuardianKeys();
  let kids = children;
  const published: Install["published"] = [];
  const gk = new GuardianKeys({
    read: () => ({ keys, children: keysChildren(kids, ["wss://default.example"]) }),
    write: (f) => {
      keys = f(keys);
    },
    workingKey: () => wSk,
    root: () => opsOf(rootSk),
    publish: async (relays, wrap) => {
      published.push({ relays, wrap });
    },
    now: () => T,
    defaultRelays: ["wss://default.example"],
  });
  return { gk, keys: () => keys, children: () => kids, setChildren: (c) => (kids = c), published };
}

/**
 * An install whose guardian identity is this phone's own key (decision
 * 1(b)): the local signer is connected, so `root()` is the local key, and a
 * fresh working key is minted on first need (D5).
 */
function phoneInstall(localSk: Uint8Array, children: Child[]): Install & { minted: () => Uint8Array | null } {
  let keys = emptyGuardianKeys();
  let kids = children;
  let minted: Uint8Array | null = null;
  const published: Install["published"] = [];
  const gk = new GuardianKeys({
    read: () => ({ keys, children: keysChildren(kids, ["wss://default.example"]) }),
    write: (f) => {
      keys = f(keys);
    },
    workingKey: () => minted ?? localSk,
    localKey: () => localSk,
    mintWorkingKey: () => (minted = generateSecretKey()),
    root: () => opsOf(localSk),
    publish: async (relays, wrap) => {
      published.push({ relays, wrap });
    },
    now: () => T,
    defaultRelays: ["wss://default.example"],
  });
  return { gk, keys: () => keys, children: () => kids, setChildren: (c) => (kids = c), published, minted: () => minted };
}

function ward(machine: string, platform: Device["platform"], label: string, guardians: StatusGuardians): Device {
  return {
    id: `dev-${label}`,
    label,
    platform,
    pairing: "paired",
    devicePubkey: machine,
    pinnedKey: R_A,
    keysViewAt: T - 100,
    appVersionCode: platform === "android" ? 55 : 804,
    guardians,
  };
}

function g(roots: { root: string; keys: string[] }[], pending: StatusGuardians["pending"] = []): StatusGuardians {
  return { roots, pending, tombstones: 0, relays: ["wss://ward.example"] };
}

/** A ward heartbeat, with only what admission and the keys view read. */
function status(machine: string, ts: number, guardians: StatusGuardians, platform: Device["platform"]): DeviceStatus {
  return { v: 1, machine, ts, appVersionCode: platform === "android" ? 55 : 804, guardians } as unknown as DeviceStatus;
}

const presenceCode = (machine: string, d: string, statement: string) =>
  `charter://presence?m=${machine}&t=${"0123456789abcdef".repeat(2)}&d=${encodeURIComponent(d)}&s=${statement}`;

let ids = 0;
const mint = () => ({
  mintChildId: () => `child-${++ids}`,
  mintDeviceId: () => `dev-${++ids}`,
  mintPolicyId: () => `pol-${++ids}`,
});

/** The store's intake for one wrap, as `wrapContext` wires it, for install B. */
function intakeFor(b: Install, sk: Uint8Array = wBSk): WrapIngestContext {
  return {
    guardianSk: sk,
    handledWraps: new Set(),
    seenReqIds: new Set(),
    hasRequest: () => false,
    ingestStatus: () => {},
    ingestRequest: () => false,
    seenAudits: new Set(),
    hasAudit: () => false,
    ingestAudit: () => false,
    ingestRoster: (inbound) => {
      const r = b.gk.receiveRoster(inbound);
      if (!r.ok) return r.reason;
      const k = b.keys();
      b.setChildren(adoptRoster(b.children(), k.roster, r.updated, { presenceWards: k.presenceWards, ...mint() }));
      b.gk.prunePresenceWards();
      return "ok";
    },
  };
}

describe("a second parent joins the family (H1, batch 3b review)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(T * 1000);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("gets the family from the roster, then signs, objects, leaves and withdraws", async () => {
    const onlyA = g([{ root: R_A, keys: [W_A] }]);
    const A = install(wASk, rASk, [
      {
        id: "a1",
        name: "Sam",
        color: "#3B6FB2",
        dependantPubkey: null,
        rosterId: SUBJECT,
        policies: [{ id: "p1", scope: { kind: "device" } }],
        devices: [ward(PHONE, "android", "Sam's phone", onlyA), ward(LAPTOP, "linux", "Sam's laptop", onlyA)],
      },
    ]);
    const B = install(wBSk, rBSk, []);

    // A invites B; B approves its phone with the invited identity.
    await A.gk.approveThisInstall({ via: "signet", label: "A's phone" });
    const invite = await A.gk.inviteCoGuardian(R_B);
    expect(invite.sent.sort()).toEqual([PHONE, LAPTOP].sort());
    const inviteD = `kintrinsic/guardian/${R_B}`;
    await B.gk.approveThisInstall({ via: "signet", label: "B's phone", expectRoot: R_B });

    // A invited R_B, so A trusts B's root; B scans A's guardian code.
    expect(A.gk.trustedRoots()).toEqual([R_A, R_B].sort());
    await expect(B.gk.confirmPresence(presenceCode(PHONE, inviteD, invite.event.id))).rejects.toThrow(/guardian code/);
    expect(B.gk.anchorRoot(`nostr:${nip19.npubEncode(R_A)}`)).toBe(R_A);

    // B confirms in person at the phone: the code is all B knows of it.
    await B.gk.confirmPresence(presenceCode(PHONE, inviteD, invite.event.id));
    expect(Object.keys(B.keys().presenceWards)).toEqual([PHONE]);

    // The phone applies it and reports both parents, to W_A and W_B.
    const both = g([
      { root: R_A, keys: [W_A] },
      { root: R_B, keys: [W_B, W_B2] },
    ]);
    const phoneStatus = status(PHONE, T + 60, both, "android");

    // The STATUS to W_B is admitted from the presence code alone (for
    // display: it vouches for nothing); a stranger isn't.
    const isPresence = (m: string) => B.gk.isPresenceWard(m);
    expect(admitStatus(phoneStatus, B.children(), new Set(), isPresence)).toBe("presence");
    expect(admitStatus(status(getPublicKey(generateSecretKey()), T, both, "android"), B.children(), new Set(), isPresence)).toBe(
      "stranger",
    );
    B.gk.notePresenceStatus(phoneStatus);
    expect(B.gk.reportedRoots()).toEqual([]);

    // A's roster reaches B and becomes B's family.
    A.setChildren(applyDeviceKeysView(A.children(), phoneStatus));
    const roster = await A.gk.sendRoster();
    expect(roster.sent).toContain(W_B);
    const toB = A.published.filter((p) => p.wrap.tags.some((t) => t[0] === "p" && t[1] === W_B)).at(-1)!.wrap;
    const ctx = intakeFor(B);
    expect(ingestWrap(toB, ctx)).toEqual({ kind: "roster" });
    expect(ctx.handledWraps.has(toB.id)).toBe(true);
    const kids = B.children();
    expect(kids).toHaveLength(1);
    expect(kids[0]).toMatchObject({ name: "Sam", color: "#3B6FB2", rosterId: SUBJECT });
    const phone = kids[0].devices.find((d) => d.devicePubkey === PHONE)!;
    const laptop = kids[0].devices.find((d) => d.devicePubkey === LAPTOP)!;
    expect(phone).toMatchObject({ fromRoster: true, pairing: "paired", platform: "android", guardians: both, keysViewAt: T + 60 });
    expect(phone.pinnedKey).toBeUndefined();
    expect(laptop).toMatchObject({ fromRoster: true, platform: "linux", label: "Sam's laptop" });
    expect(laptop.keysViewAt).toBeUndefined();
    expect(B.keys().presenceWards).toEqual({});
    // Only the laptop is still to confirm; the phone has reported.
    expect(B.gk.unconfirmedRosterDevices().map((d) => d.machine)).toEqual([LAPTOP]);
    expect(admitStatus(phoneStatus, B.children(), new Set(), isPresence)).toBe("claimed");
    // B can sign for the phone with W at once: the laptop, known only from
    // the roster and not heard from, doesn't hold the child back (N-L1).
    expect(B.gk.keyFor(PHONE)).toBe("working");
    expect(B.gk.familyReadiness().ready).toBe(true);

    // B confirms at the laptop too (now a known device: no presence ward).
    await B.gk.confirmPresence(presenceCode(LAPTOP, inviteD, invite.event.id));
    expect(B.keys().presenceWards).toEqual({});
    B.setChildren(applyDeviceKeysView(B.children(), status(LAPTOP, T + 90, both, "linux")));
    expect(B.gk.unconfirmedRosterDevices()).toEqual([]);

    // B can set a rule: W_B signs, and the wards list W_B under R_B.
    expect(B.gk.keyFor(PHONE)).toBe("working");
    expect(B.gk.keyFor(LAPTOP)).toBe("working");

    // A starts removing B; B sees it as a removal of B, and objects.
    const removalD = `kintrinsic/unguardian/${R_B}`;
    const withRemoval = g(both.roots, [{ d: removalD, kind: "removal", takesEffectAt: T + 72 * 3600, expiresAt: T + 7 * 86400 }]);
    B.setChildren(applyDeviceKeysView(B.children(), status(PHONE, T + 120, withRemoval, "android")));
    B.setChildren(applyDeviceKeysView(B.children(), status(LAPTOP, T + 120, withRemoval, "linux")));
    const wards = familyWards(B.children());
    expect(pendingFor(wards[0].device.guardians, B.keys(), R_B, T + 120)[0].role).toBe("against-me");
    expect(removalsAgainstMe(wards, B.keys(), R_B, T + 120).map((r) => r.d)).toEqual([removalD]);
    const objection = await B.gk.objectToRemoval(removalD);
    expect(objection.sent.sort()).toEqual([PHONE, LAPTOP].sort());
    const parsed = parseStatement(objection.event as SignedEvent);
    expect(parsed.ok && parsed.statement.signer).toBe(R_B);
    expect(objection.event.tags.find((t) => t[0] === "p")?.[1]).toBe(R_A);

    // B can leave: every ward reports another guardian.
    expect(leaveGate(wards, R_B)).toEqual({ ok: true });
    const leave = await B.gk.leaveFamily();
    expect(leave.sent.sort()).toEqual([PHONE, LAPTOP].sort());

    // B can withdraw its own other phone.
    const withdraw = await B.gk.withdrawInstall(W_B2);
    expect(withdraw.sent.sort()).toEqual([PHONE, LAPTOP].sort());
    const opened = unwrapToWorkingKey(B.published.at(-1)!.wrap, B.published.at(-1)!.wrap.tags[0][1] === PHONE ? phoneSk : laptopSk);
    expect(opened?.sealAuthor).toBe(W_B);
  });

  it("with this phone only: its key is the identity, a fresh day-to-day key confirms with the approval inline", async () => {
    const onlyA = g([{ root: R_A, keys: [W_A] }]);
    const A = install(wASk, rASk, [
      {
        id: "a1",
        name: "Sam",
        color: "#3B6FB2",
        dependantPubkey: null,
        rosterId: SUBJECT,
        policies: [{ id: "p1", scope: { kind: "device" } }],
        devices: [ward(PHONE, "android", "Sam's phone", onlyA), ward(LAPTOP, "linux", "Sam's laptop", onlyA)],
      },
    ]);
    const r2Sk = generateSecretKey();
    const R2 = getPublicKey(r2Sk);
    const B = phoneInstall(r2Sk, []);
    // A My Signet once connected and dropped on this phone names no device here.
    B.gk["deps"].write((k) => ({ ...k, forgottenRoot: R_B }));

    // B joins with this phone only: R2 is this phone's key, and it approves a fresh W.
    const made = await B.gk.joinWithThisPhone();
    const wSk = B.minted()!;
    const W = getPublicKey(wSk);
    expect(W).not.toBe(R2);
    expect(made).toMatchObject({ pubkey: R2, via: "phone" });
    expect(B.keys().forgottenRoot).toBeUndefined();
    const approval = parseStatement(made.approval);
    expect(approval.ok && approval.statement).toMatchObject({ type: "approval", signer: R2, key: W, d: `kintrinsic/device/${W}` });
    expect(B.gk.workingPubkey()).toBe(W);
    expect(B.gk.rootPubkey()).toBe(R2);
    expect(B.published).toEqual([]); // no device to send the approval to yet
    // Asking again keeps the same identity and key.
    expect((await B.gk.joinWithThisPhone()).approval.id).toBe(made.approval.id);
    expect(B.minted()).toBe(wSk);

    // B's guardian code carries R2; A scans it and invites it, exactly as an npub from My Signet.
    const code = `nostr:${nip19.npubEncode(R2)}`;
    await A.gk.approveThisInstall({ via: "signet", label: "A's phone" });
    const invite = await A.gk.inviteCoGuardian(code.replace(/^nostr:/, ""));
    const inviteD = `kintrinsic/guardian/${R2}`;
    const st = parseStatement(invite.event as SignedEvent);
    expect(st.ok && st.statement).toMatchObject({ type: "coguardian", signer: R_A, key: R2, d: inviteD });
    expect(invite.event.tags.find((t) => t[0] === "p")?.[1]).toBe(R2);
    expect(A.gk.trustedRoots()).toEqual([R_A, R2].sort());

    // Its own code, pasted where the other parent's goes: a hint true for a phone-held identity.
    expect(() => B.gk.anchorRoot(code)).toThrow("That's this phone's own code. Ask the other parent for theirs.");
    expect(() => B.gk.anchorRoot(code)).not.toThrow(/second phone/);
    // B scans A's code, then confirms in person: W signs and seals, with R2's approval of W inline (rule 6).
    expect(B.gk.anchorRoot(`nostr:${nip19.npubEncode(R_A)}`)).toBe(R_A);
    const confirm = await B.gk.confirmPresence(presenceCode(PHONE, inviteD, invite.event.id));
    expect(confirm.sent).toEqual([PHONE]);
    const opened = unwrapToWorkingKey(B.published.at(-1)!.wrap, phoneSk)!;
    expect(opened.sealAuthor).toBe(W);
    expect(opened.inner).toMatchObject({ kind: 31121, pubkey: W });
    const content = JSON.parse(opened.inner.content) as { v: number; token: string; statement: string; approval: SignedEvent };
    expect(content).toMatchObject({ v: 1, token: "0123456789abcdef".repeat(2), statement: invite.event.id });
    expect(content.approval.id).toBe(made.approval.id);
    expect(content.approval.pubkey).toBe(R2);
    expect(content.approval.tags).toContainEqual(["p", W]);
    expect(Object.keys(B.keys().presenceWards)).toEqual([PHONE]);

    // The phone pins R2 and binds W under it, and reports both parents.
    const both = g([
      { root: R_A, keys: [W_A] },
      { root: R2, keys: [W] },
    ]);
    const phoneStatus = status(PHONE, T + 60, both, "android");
    B.gk.notePresenceStatus(phoneStatus);

    // A's roster reaches W and becomes B's family, as for a My Signet joiner.
    A.setChildren(applyDeviceKeysView(A.children(), phoneStatus));
    const roster = await A.gk.sendRoster();
    expect(roster.sent).toEqual([W]);
    const toB = A.published.filter((p) => p.wrap.tags.some((t) => t[0] === "p" && t[1] === W)).at(-1)!.wrap;
    expect(ingestWrap(toB, intakeFor(B, wSk))).toEqual({ kind: "roster" });
    expect(B.children()).toHaveLength(1);
    expect(B.children()[0]).toMatchObject({ name: "Sam", rosterId: SUBJECT });
    expect(B.keys().presenceWards).toEqual({});

    // R2 keeps signing the rules itself until a move (D5); W carries R2's statements where listed.
    expect(B.gk.keyFor(PHONE)).toBe("root");
    expect(B.gk.opsFor(PHONE).pubkey).toBe(R2);

    // B's own roster carries R2's approval of W, and A takes it.
    const back = await B.gk.sendRoster();
    expect(back.sent).toEqual([W_A]);
    const toA = B.published.filter((p) => p.wrap.tags.some((t) => t[0] === "p" && t[1] === W_A)).at(-1)!.wrap;
    const fromB = unwrapToWorkingKey(toA, wASk)!;
    expect(fromB.sealAuthor).toBe(W);
    expect(A.gk.receiveRoster(fromB)).toMatchObject({ ok: true });

    // The move to My Signet later is open to B, as to any phone-held identity.
    expect(B.gk.familyReadiness().ready).toBe(true);
  });

  it("with this phone only: refused on a phone that looks after devices, or whose identity is My Signet", async () => {
    const onlyMine = g([{ root: R_B, keys: [] }]);
    const busy = phoneInstall(rBSk, [
      { id: "b1", name: "Kit", color: "#000000", dependantPubkey: null, policies: [], devices: [ward(PHONE, "android", "Kit's phone", onlyMine)] },
    ]);
    await expect(busy.gk.joinWithThisPhone()).rejects.toThrow(/already looks after devices/);
    expect(busy.minted()).toBeNull();

    const signet = install(wBSk, rBSk, []);
    await signet.gk.approveThisInstall({ via: "signet", label: "B's phone" });
    await expect(signet.gk.joinWithThisPhone()).rejects.toThrow(/already uses My Signet/);
    // A My Signet identity keeps the second-phone hint.
    expect(() => signet.gk.anchorRoot(nip19.npubEncode(R_B))).toThrow(/For a second phone of your own, skip this step/);
  });

  it("a roster child that shares a device with one this install added keeps one child, under the smaller id", () => {
    const mine: Child = {
      id: "c-local",
      name: "Sammy",
      color: "#000000",
      dependantPubkey: null,
      rosterId: "ff".repeat(32),
      policies: [],
      devices: [ward(PHONE, "android", "Phone", g([{ root: R_A, keys: [W_A] }]))],
    };
    const roster = {
      children: [
        {
          subject: SUBJECT,
          issuedAt: T,
          name: "Sam",
          colour: "#3B6FB2",
          devices: [
            { machine: PHONE, platform: "android", name: "Sam's phone" },
            { machine: LAPTOP, platform: "linux", name: "Sam's laptop" },
          ],
        },
      ],
    };
    const out = adoptRoster([mine], roster, [SUBJECT], mint());
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ id: "c-local", name: "Sam", rosterId: SUBJECT });
    // The device paired here keeps its record; the laptop is added from the roster.
    expect(out[0].devices.map((d) => [d.devicePubkey, !!d.fromRoster])).toEqual([
      [PHONE, false],
      [LAPTOP, true],
    ]);
    // A newer roster that drops the unheard laptop drops it; the phone stays.
    const later = { children: [{ ...roster.children[0], issuedAt: T + 1, devices: [] }] };
    expect(adoptRoster(out, later, [SUBJECT], mint())[0].devices.map((d) => d.devicePubkey)).toEqual([PHONE]);
  });
});
