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

function ward(machine: string, platform: Device["platform"], label: string, guardians: StatusGuardians): Device {
  return {
    id: `dev-${label}`,
    label,
    platform,
    pairing: "paired",
    devicePubkey: machine,
    pinnedKey: R_A,
    keysViewAt: T - 100,
    appVersionCode: platform === "android" ? 49 : 713,
    guardians,
  };
}

function g(roots: { root: string; keys: string[] }[], pending: StatusGuardians["pending"] = []): StatusGuardians {
  return { roots, pending, tombstones: 0, relays: ["wss://ward.example"] };
}

/** A ward heartbeat, with only what admission and the keys view read. */
function status(machine: string, ts: number, guardians: StatusGuardians, platform: Device["platform"]): DeviceStatus {
  return { v: 1, machine, ts, appVersionCode: platform === "android" ? 49 : 713, guardians } as unknown as DeviceStatus;
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
function intakeFor(b: Install): WrapIngestContext {
  return {
    guardianSk: wBSk,
    handledWraps: new Set(),
    seenReqIds: new Set(),
    hasRequest: () => false,
    ingestStatus: () => {},
    ingestRequest: () => false,
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
    expect(pendingFor(wards[0].device.guardians, B.keys(), R_B)[0].role).toBe("against-me");
    expect(removalsAgainstMe(wards, B.keys(), R_B).map((r) => r.d)).toEqual([removalD]);
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
