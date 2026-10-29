// The guardian app's guardian-keys actions, tested against the contract
// (spec/contract.md, *Guardian roots and working keys (v1)*): statement
// shapes, who signs and who seals each one, the versioning gate, presence
// confirmation, the PIN and the roster. Wards are stood in for by their own
// secret keys, which open each wrap exactly as a ward would.

import { beforeEach, describe, expect, it } from "vitest";
import { finalizeEvent, generateSecretKey, getPublicKey, nip19, nip44, verifyEvent, type NostrEvent } from "nostr-tools";
import { GuardianKeys, unwrapToWorkingKey, type KeysChild, type KeysDevice } from "./guardianKeys";
import { emptyGuardianKeys, type GuardianKeysState } from "./state";
import { guardianPinHash } from "./pin";
import { parseStatement, CHARTER_PRESENCE_CONFIRM, CHARTER_FAMILY_ROSTER, type SignedEvent } from "./sdk";
import type { GuardianOps } from "../wire/giftwrap";

const T = 1_800_000_000;

function opsOf(sk: Uint8Array): GuardianOps {
  return {
    pubkey: getPublicKey(sk),
    signEvent: async (t) => finalizeEvent(t, sk),
    nip44Encrypt: async (to, pt) => nip44.encrypt(pt, nip44.getConversationKey(sk, to)),
  };
}

interface Harness {
  gk: GuardianKeys;
  keys: () => GuardianKeysState;
  children: KeysChild[];
  published: { relays: string[]; wrap: NostrEvent }[];
  resigned: string[];
  setRoot: (ops: GuardianOps | null) => void;
}

const rSk = generateSecretKey();
const wSk = generateSecretKey();
const r2Sk = generateSecretKey();
const R = getPublicKey(rSk);
const W = getPublicKey(wSk);
const R2 = getPublicKey(r2Sk);
const phoneSk = generateSecretKey();
const laptopSk = generateSecretKey();
const PHONE = getPublicKey(phoneSk);
const LAPTOP = getPublicKey(laptopSk);

function device(machine: string, platform: "android" | "linux", extra: Partial<KeysDevice> = {}): KeysDevice {
  return { machine, platform, name: platform === "android" ? "Sam's phone" : "Sam's laptop", relays: ["wss://a.example", "wss://b.example"], statusSeen: false, ...extra };
}

function harness(children: KeysChild[], rootOps: GuardianOps | null = opsOf(rSk)): Harness {
  let keys = emptyGuardianKeys();
  let root = rootOps;
  const published: Harness["published"] = [];
  const resigned: string[] = [];
  const gk = new GuardianKeys({
    read: () => ({ keys, children }),
    write: (f) => {
      keys = f(keys);
    },
    workingKey: () => wSk,
    root: () => root,
    publish: async (relays, wrap) => {
      published.push({ relays, wrap });
    },
    now: () => T,
    defaultRelays: ["wss://default.example"],
    resignClauses: async (id) => {
      resigned.push(id);
    },
  });
  return { gk, keys: () => keys, children, published, resigned, setRoot: (o) => (root = o) };
}

/** Open a wrap as the ward with `sk` would. */
function open(wrap: NostrEvent, sk: Uint8Array) {
  const out = unwrapToWorkingKey(wrap, sk);
  if (!out) throw new Error("wrap did not open");
  expect(verifyEvent(out.inner as NostrEvent)).toBe(true);
  return out;
}

const wardSk = (m: string) => (m === PHONE ? phoneSk : laptopSk);
const recipient = (w: NostrEvent) => w.tags.find((t) => t[0] === "p")![1];

function tag(e: SignedEvent, name: string) {
  return e.tags.filter((t) => t[0] === name).map((t) => t[1]);
}

/** A ready, section-implementing ward that holds W under R (and `extraRoots`). */
function readyWard(machine: string, platform: "android" | "linux", extraRoots: { root: string; keys: string[] }[] = []) {
  return device(machine, platform, {
    statusSeen: true,
    appVersionCode: platform === "android" ? 49 : 713,
    guardians: { roots: [{ root: R, keys: [W] }, ...extraRoots], pending: [], tombstones: 0, relays: ["wss://ward.example", "wss://b.example"] },
  });
}

async function connectSignetRoot(h: Harness) {
  return h.gk.approveThisInstall({ via: "signet", label: "Parent's phone" });
}

describe("connecting My Signet as root", () => {
  it("asks R once for an approval of W and stores it", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [device(PHONE, "android")] }]);
    const root = await connectSignetRoot(h);
    expect(root.pubkey).toBe(R);
    const a = root.approval;
    expect(a.kind).toBe(31120);
    expect(a.pubkey).toBe(R);
    expect(tag(a, "d")).toEqual([`kintrinsic/device/${W}`]);
    expect(tag(a, "p")).toEqual([W]);
    expect(tag(a, "t")).toEqual(["charter-device"]);
    expect(JSON.parse(a.content)).toEqual({ v: 1, issuedAt: T, label: "Parent's phone" });
    expect(h.keys().root).toEqual({ pubkey: R, approval: a, via: "signet" });
    expect(h.gk.rootPubkey()).toBe(R);
  });

  it("refuses a bunker whose key is W itself, or the wrong identity", async () => {
    const h = harness([], opsOf(wSk));
    await expect(connectSignetRoot(h)).rejects.toThrow(/can't approve itself/);
    const h2 = harness([]);
    await expect(h2.gk.approveThisInstall({ via: "signet", label: "x", expectRoot: R2 })).rejects.toThrow(/isn't the one/);
  });

  it("refuses something other than R's approval of W", async () => {
    const liar: GuardianOps = { ...opsOf(rSk), signEvent: async (t) => finalizeEvent({ ...t, kind: 1 }, rSk) };
    const h = harness([], liar);
    await expect(connectSignetRoot(h)).rejects.toThrow(/other than an approval/);
    expect(h.keys().root).toBeUndefined();
  });

  it("a local-only family names its local key as the root", () => {
    const h = harness([], opsOf(wSk));
    expect(h.gk.rootPubkey()).toBe(W);
  });
});

describe("approval delivery", () => {
  it("sends W's approval, sealed by W, to each ward that doesn't list W, then waits", async () => {
    const child: KeysChild = {
      id: "c1",
      name: "Sam",
      colour: "#000",
      devices: [device(PHONE, "android"), readyWard(LAPTOP, "linux")],
    };
    const h = harness([child]);
    await connectSignetRoot(h);
    expect(await h.gk.syncApprovals()).toEqual([PHONE]);
    expect(h.published).toHaveLength(1);
    const { sealAuthor, inner } = open(h.published[0].wrap, phoneSk);
    expect(sealAuthor).toBe(W); // exception 1: sealed by the W it names
    expect(JSON.parse(JSON.stringify(inner))).toEqual(h.keys().root!.approval);
    expect(h.published[0].relays).toEqual(["wss://a.example", "wss://b.example"]);
    // Rate-limited, unless forced.
    expect(await h.gk.syncApprovals()).toEqual([]);
    expect(await h.gk.syncApprovals({ force: true })).toEqual([PHONE]);
  });

  it("sends nothing for a local-only family", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [device(PHONE, "android")] }], opsOf(wSk));
    expect(await h.gk.syncApprovals()).toEqual([]);
  });
});

describe("which key signs for a ward", () => {
  it("local-only families sign with the root, the local key", () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] }], opsOf(wSk));
    expect(h.gk.keyFor(PHONE)).toBe("root");
    expect(h.gk.opsFor(PHONE).pubkey).toBe(W);
  });

  it("W signs only where the ward lists W under R and every ward of the child is ready", async () => {
    const child: KeysChild = { id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] };
    const h = harness([child]);
    expect(h.gk.keyFor(PHONE)).toBe("root"); // no Signet root yet
    await connectSignetRoot(h);
    expect(h.gk.keyFor(PHONE)).toBe("working");
    expect(h.gk.opsFor(PHONE).pubkey).toBe(W);

    // A silent sibling ward (no STATUS ever) closes the gate for the child.
    child.devices.push(device(LAPTOP, "linux"));
    expect(h.gk.keyFor(PHONE)).toBe("root");
    expect(h.gk.opsFor(PHONE).pubkey).toBe(R);
    expect(h.gk.childReadiness("c1").wards[1].readiness).toEqual({ ready: false, reason: "silent" });

    // An old sibling ward also closes it.
    child.devices[1] = device(LAPTOP, "linux", { statusSeen: true, appVersionCode: 712 });
    expect(h.gk.keyFor(PHONE)).toBe("root");
    child.devices[1] = readyWard(LAPTOP, "linux");
    expect(h.gk.keyFor(PHONE)).toBe("working");
  });

  it("R signs for a ward without guardians, or one that lists W under another root", async () => {
    const child: KeysChild = {
      id: "c1",
      name: "Sam",
      colour: "#000",
      devices: [device(PHONE, "android", { statusSeen: true, appVersionCode: 60 })],
    };
    const h = harness([child]);
    await connectSignetRoot(h);
    expect(h.gk.keyFor(PHONE)).toBe("root");
    child.devices[0] = { ...readyWard(PHONE, "android"), guardians: { roots: [{ root: R2, keys: [W] }], pending: [], tombstones: 0, relays: [] } };
    expect(h.gk.keyFor(PHONE)).toBe("root");
  });

  it("W signs where the ward pins W itself as a root", async () => {
    const child: KeysChild = {
      id: "c1",
      name: "Sam",
      colour: "#000",
      devices: [{ ...readyWard(PHONE, "android"), guardians: { roots: [{ root: W, keys: [] }], pending: [], tombstones: 0, relays: [] } }],
    };
    const h = harness([child]);
    await connectSignetRoot(h);
    expect(h.gk.keyFor(PHONE)).toBe("working");
  });

  it("PAIR_OFFER is sealed by W and carries the approval only with a Signet root", async () => {
    const h = harness([]);
    expect(h.gk.pairOffer("ab".repeat(16), ["wss://x.example"], T)).toBeNull();
    await connectSignetRoot(h);
    const offer = h.gk.pairOffer("ab".repeat(16), ["wss://x.example"], T)!;
    expect(offer.ops.pubkey).toBe(W);
    expect(JSON.parse(offer.content)).toEqual({
      token: "ab".repeat(16),
      relays: ["wss://x.example"],
      ts: T,
      approval: h.keys().root!.approval,
    });
  });
});

describe("statements", () => {
  let h: Harness;
  let child: KeysChild;

  beforeEach(async () => {
    child = {
      id: "c1",
      name: "Sam",
      colour: "#000",
      // The phone holds W under R; the laptop is ready but doesn't hold W yet.
      devices: [
        readyWard(PHONE, "android", [{ root: R2, keys: [] }]),
        { ...readyWard(LAPTOP, "linux"), guardians: { roots: [{ root: R, keys: [] }, { root: R2, keys: [] }], pending: [], tombstones: 0, relays: [] } },
      ],
    };
    h = harness([child]);
    await connectSignetRoot(h);
  });

  function delivered() {
    return h.published.map((p) => ({ to: recipient(p.wrap), ...open(p.wrap, wardSk(recipient(p.wrap))) }));
  }

  it("co-guardian: signed by R, sealed by W where the ward holds W, else by R", async () => {
    const report = await h.gk.inviteCoGuardian(nip19.npubEncode(R2), { label: "Alex" });
    expect(report.sent.sort()).toEqual([PHONE, LAPTOP].sort());
    const out = delivered();
    expect(out).toHaveLength(2);
    for (const o of out) {
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "coguardian", d: `kintrinsic/guardian/${R2}`, p: R2, signer: R, label: "Alex" });
      expect(o.sealAuthor).toBe(o.to === PHONE ? W : R);
    }
    expect(h.keys().sent[`kintrinsic/guardian/${R2}`]).toMatchObject({ type: "coguardian", key: R2, id: report.event.id });
  });

  it("co-guardian is refused while any ward is not ready, naming it", async () => {
    child.devices.push(device("e5".repeat(32), "android", { name: "Old tablet", statusSeen: true, appVersionCode: 40 }));
    await expect(h.gk.inviteCoGuardian(R2)).rejects.toThrow(/Update Kintrinsic on Old tablet/);
    expect(h.published).toHaveLength(0);
  });

  it("co-guardian refuses bad and own ids", async () => {
    await expect(h.gk.inviteCoGuardian("npub1nope")).rejects.toThrow(/guardian ID/);
    await expect(h.gk.inviteCoGuardian(R)).rejects.toThrow(/your own/);
  });

  it("leaving: a removal of this root signed by it; refused for the last root", async () => {
    await h.gk.leaveFamily();
    for (const o of delivered()) {
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "removal", d: `kintrinsic/unguardian/${R}`, p: R, signer: R });
    }
    child.devices[1] = readyWard(LAPTOP, "linux"); // only R pinned there
    await expect(h.gk.leaveFamily()).rejects.toThrow(/only guardian/);
  });

  it("leaving is refused when no ward reports its guardians", async () => {
    child.devices = [device(PHONE, "android")];
    await expect(h.gk.leaveFamily()).rejects.toThrow(/reported its guardians/);
  });

  it("removing the other parent re-signs the clauses first (rule 4)", async () => {
    await h.gk.removeCoGuardian(R2);
    expect(h.resigned).toEqual(["c1"]);
    for (const o of delivered()) {
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "removal", d: `kintrinsic/unguardian/${R2}`, p: R2, signer: R });
    }
    await expect(h.gk.removeCoGuardian(R)).rejects.toThrow(/leave the family/);
  });

  it("a failed re-sign aborts the removal", async () => {
    const hh = new GuardianKeys({
      read: () => ({ keys: h.keys(), children: [child] }),
      write: () => {},
      workingKey: () => wSk,
      root: () => opsOf(rSk),
      publish: async () => {
        throw new Error("must not publish");
      },
      now: () => T,
      defaultRelays: [],
      resignClauses: async () => {
        throw new Error("signer unreachable");
      },
    });
    await expect(hh.removeCoGuardian(R2)).rejects.toThrow(/signer unreachable/);
  });

  it("objection: signed AND sealed by the root being removed, p = the remover", async () => {
    const d = `kintrinsic/unguardian/${R}`;
    for (const dev of child.devices) dev.guardians!.pending = [{ d, kind: "removal", expiresAt: T + 100 }];
    await h.gk.objectToRemoval(d);
    for (const o of delivered()) {
      expect(o.sealAuthor).toBe(R); // never W: the objection isn't in exception 2
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "objection", d: `kintrinsic/object/${d}`, p: R2, signer: R });
    }
  });

  it("objection needs a removal of this root and an unambiguous remover", async () => {
    await expect(h.gk.objectToRemoval(`kintrinsic/unguardian/${R2}`)).rejects.toThrow(/being removed/);
    await expect(h.gk.objectToRemoval(`kintrinsic/guardian/${R}`)).rejects.toThrow(/pending removal/);
    const d = `kintrinsic/unguardian/${R}`;
    const R3 = getPublicKey(generateSecretKey());
    child.devices[0].guardians!.roots.push({ root: R3, keys: [] });
    child.devices[0].guardians!.pending = [{ d, kind: "removal", expiresAt: T + 100 }];
    await expect(h.gk.objectToRemoval(d)).rejects.toThrow(/which parent/);
    await h.gk.objectToRemoval(d, nip19.npubEncode(R3));
    const st = parseStatement(open(h.published[0].wrap, wardSk(recipient(h.published[0].wrap))).inner);
    expect(st.ok && st.statement.p).toBe(R3);
  });

  it("cancel: signed by R, names the pending statement's d and key", async () => {
    const d = `kintrinsic/guardian/${R2}`;
    await h.gk.cancelPending(d);
    for (const o of delivered()) {
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "cancel", d: `kintrinsic/cancel/${d}`, p: R2, signer: R });
      expect(o.sealAuthor).toBe(o.to === PHONE ? W : R);
    }
    await expect(h.gk.cancelPending(`kintrinsic/device/${W}`)).rejects.toThrow(/isn't a pending/);
  });

  it("withdrawing another install: signed by R, sealed by this W; re-signs first", async () => {
    const W2 = getPublicKey(generateSecretKey());
    child.devices[0].guardians!.roots[0].keys.push(W2);
    await h.gk.withdrawInstall(W2);
    expect(h.resigned).toEqual(["c1"]);
    for (const o of delivered()) {
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "withdrawal", d: `kintrinsic/revoke/${W2}`, p: W2, signer: R });
      expect(o.sealAuthor).toBe(o.to === PHONE ? W : R);
    }
  });

  it("withdrawing this install's own W is sealed by R itself", async () => {
    await h.gk.withdrawInstall(W);
    expect(h.resigned).toEqual([]);
    for (const o of delivered()) expect(o.sealAuthor).toBe(R);
  });

  it("refuses to withdraw another parent's install", async () => {
    const W3 = getPublicKey(generateSecretKey());
    child.devices[0].guardians!.roots[1].keys.push(W3);
    await expect(h.gk.withdrawInstall(W3)).rejects.toThrow(/another parent/);
  });

  it("R signs over its own connection: no root, no statement", async () => {
    h.setRoot(null);
    await expect(h.gk.cancelPending(`kintrinsic/guardian/${R2}`)).rejects.toThrow(/Connect/);
    h.setRoot(opsOf(r2Sk));
    await expect(h.gk.cancelPending(`kintrinsic/guardian/${R2}`)).rejects.toThrow(/isn't this family's/);
  });
});

describe("presence confirmation", () => {
  const token = "0123456789abcdef0123456789abcdef";
  const stmt = "ef".repeat(32);

  it("co-guardian: kind 31121 signed and sealed by W, carrying R2's approval of W", async () => {
    // The joining install: its root is R2 (so here, "R" is the joiner's root).
    const h = harness([]);
    await connectSignetRoot(h);
    const code = `charter://presence?m=${PHONE}&t=${token}&d=${encodeURIComponent(`kintrinsic/guardian/${R}`)}&s=${stmt}`;
    const report = await h.gk.confirmPresence(code);
    expect(report.sent).toEqual([PHONE]);
    expect(h.published[0].relays).toEqual(["wss://default.example"]); // an unknown ward: defaults
    const { sealAuthor, inner } = open(h.published[0].wrap, phoneSk);
    expect(sealAuthor).toBe(W);
    expect(inner.pubkey).toBe(W);
    expect(inner.kind).toBe(CHARTER_PRESENCE_CONFIRM);
    expect(tag(inner, "t")).toEqual(["charter-device"]);
    expect(JSON.parse(inner.content)).toEqual({ v: 1, token, statement: stmt, approval: h.keys().root!.approval });
  });

  it("refuses a co-guardian invitation for another identity, and bad codes", async () => {
    const h = harness([]);
    await connectSignetRoot(h);
    const code = `charter://presence?m=${PHONE}&t=${token}&d=kintrinsic/guardian/${R2}&s=${stmt}`;
    await expect(h.gk.confirmPresence(code)).rejects.toThrow(/different guardian ID/);
    await expect(h.gk.confirmPresence("charter://presence?m=zz")).rejects.toThrow(/presence code/);
    expect(h.published).toHaveLength(0);
  });

  it("removal: no approval; sent on the ward's known relays", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] }]);
    await connectSignetRoot(h);
    await h.gk.confirmPresence(`charter://presence?m=${PHONE}&t=${token}&d=kintrinsic/unguardian/${R2}&s=${stmt}`);
    const { inner } = open(h.published[0].wrap, phoneSk);
    expect(JSON.parse(inner.content)).toEqual({ v: 1, token, statement: stmt });
    expect(h.published[0].relays.sort()).toEqual(["wss://a.example", "wss://b.example", "wss://ward.example"]);
  });
});

describe("root move", () => {
  it("R signs the move, R2 approves W, and the confirmation carries R2's approval", async () => {
    const child: KeysChild = { id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] };
    const h = harness([child]);
    await connectSignetRoot(h);
    await h.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "bunker://r2", via: "heartwood" });
    expect(h.resigned).toEqual(["c1"]);
    const moved = open(h.published[0].wrap, phoneSk);
    expect(moved.sealAuthor).toBe(W);
    const st = parseStatement(moved.inner);
    expect(st.ok && st.statement).toMatchObject({ type: "move", d: `kintrinsic/root/${R2}`, p: R2, signer: R });
    const next = h.keys().nextRoot!;
    expect(next).toMatchObject({ pubkey: R2, via: "heartwood", bunkerUri: "bunker://r2", moveD: `kintrinsic/root/${R2}`, moveId: moved.inner.id });
    const ap = parseStatement(next.approval);
    expect(ap.ok && ap.statement).toMatchObject({ type: "approval", signer: R2, key: W });

    const token = "0123456789abcdef0123456789abcdef";
    await h.gk.confirmPresence(`charter://presence?m=${PHONE}&t=${token}&d=kintrinsic/root/${R2}&s=${moved.inner.id}`);
    const conf = open(h.published[1].wrap, phoneSk);
    expect(JSON.parse(conf.inner.content).approval).toEqual(next.approval);

    // Not settled while the ward still pins R; settled once it pins R2 only.
    expect(h.gk.settleRootMove()).toBeNull();
    child.devices[0].guardians = { roots: [{ root: R2, keys: [W] }], pending: [], tombstones: 0, relays: [] };
    expect(h.gk.settleRootMove()).toBe("bunker://r2");
    expect(h.keys().root).toEqual({ pubkey: R2, approval: next.approval, via: "heartwood" });
    expect(h.keys().nextRoot).toBeUndefined();
  });

  it("is refused for a local-only family and while a ward isn't ready", async () => {
    const child: KeysChild = { id: "c1", name: "Sam", colour: "#000", devices: [device(PHONE, "android")] };
    const local = harness([child], opsOf(wSk));
    await expect(local.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "x", via: "signet" })).rejects.toThrow(/isn't available/);
    const h = harness([child]);
    await connectSignetRoot(h);
    await expect(h.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "x", via: "signet" })).rejects.toThrow(/Update Kintrinsic/);
    expect(h.published).toHaveLength(0);
  });
});

describe("guardian PIN", () => {
  it("sends a gift-wrapped RELAYS with guardianPin to ready Android wards only", async () => {
    const old = "f6".repeat(32);
    const child: KeysChild = {
      id: "c1",
      name: "Sam",
      colour: "#000",
      devices: [readyWard(PHONE, "android"), readyWard(LAPTOP, "linux")],
    };
    // A sibling's old tablet: its own child, so Sam's gate stays open.
    const sibling: KeysChild = {
      id: "c2",
      name: "Jo",
      colour: "#111",
      devices: [device(old, "android", { name: "Old tablet", statusSeen: true, appVersionCode: 48 })],
    };
    const h = harness([child, sibling]);
    await connectSignetRoot(h);
    const report = await h.gk.setGuardianPin("123456");
    expect(report.sent).toEqual([PHONE]);
    expect(report.failed).toEqual([{ to: old, error: "Update Kintrinsic on Old tablet first." }]);
    const { sealAuthor, inner } = open(h.published[0].wrap, phoneSk);
    expect(sealAuthor).toBe(W);
    expect(inner.pubkey).toBe(W); // the ward's usual key: W, listed under R
    expect(inner.kind).toBe(31118);
    const body = JSON.parse(inner.content);
    expect(body.machine).toBe(PHONE);
    expect(body.relays).toEqual(["wss://ward.example", "wss://b.example"]);
    expect(await guardianPinHash("123456", body.guardianPin.salt)).toBe(body.guardianPin.hash);
    expect(h.keys().pinSentAt).toEqual({ [PHONE]: T });
    expect(JSON.stringify(h.keys())).not.toContain("123456");
  }, 30_000);
});

describe("family roster", () => {
  it("goes W to W with the approval, and merges on the other install", async () => {
    const W2sk = generateSecretKey();
    const W2 = getPublicKey(W2sk);
    const guardians = { roots: [{ root: R, keys: [W] }, { root: R2, keys: [W2] }], pending: [], tombstones: 0, relays: [] };
    const subject = "aa".repeat(32);
    const child: KeysChild = {
      id: "c1",
      name: "Sam",
      colour: "#3B6FB2",
      rosterId: subject,
      devices: [{ ...readyWard(PHONE, "android"), guardians }],
    };
    const h = harness([child]);
    await connectSignetRoot(h);
    const report = await h.gk.sendRoster();
    expect(report.sent).toEqual([W2]);
    const opened = unwrapToWorkingKey(h.published[0].wrap, W2sk)!;
    expect(opened.sealAuthor).toBe(W);
    expect(opened.inner.kind).toBe(CHARTER_FAMILY_ROSTER);
    const content = JSON.parse(opened.inner.content);
    expect(content.v).toBe(1);
    expect(content.approval).toEqual(h.keys().root!.approval);
    expect(content.children).toEqual([
      { subject, issuedAt: T, name: "Sam", colour: "#3B6FB2", devices: [{ machine: PHONE, platform: "android", name: "Sam's phone" }] },
    ]);

    // The other parent's install (W2) takes it in.
    let keys2 = emptyGuardianKeys();
    const other = new GuardianKeys({
      read: () => ({ keys: keys2, children: [{ id: "x", name: "?", colour: "#000", devices: [{ ...readyWard(LAPTOP, "linux"), guardians }] }] }),
      write: (f) => {
        keys2 = f(keys2);
      },
      workingKey: () => W2sk,
      root: () => opsOf(r2Sk),
      publish: async () => {},
      now: () => T,
      defaultRelays: [],
    });
    expect(other.receiveRoster(opened)).toEqual({ ok: true, updated: [subject] });
    expect(other.unconfirmedRosterDevices()).toEqual([{ subject, machine: PHONE, name: "Sam's phone", platform: "android", pairedHere: false, statusSeen: false }]);
    // A roster from a key the wards don't list is refused.
    const strangerSk = generateSecretKey();
    const forged = { sealAuthor: getPublicKey(strangerSk), inner: finalizeEvent({ ...opened.inner, created_at: T }, strangerSk) as SignedEvent };
    expect(other.receiveRoster(forged)).toMatchObject({ ok: false });
    // Its own roster is ignored.
    expect(h.gk.receiveRoster(opened)).toEqual({ ok: false, reason: "own" });

    // Unchanged records keep their issuedAt on the next send.
    await h.gk.sendRoster();
    expect(h.keys().roster.children[0].issuedAt).toBe(T);
  });

  it("a local-only install has no approval to carry and sends none", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", rosterId: "aa".repeat(32), devices: [readyWard(PHONE, "android")] }], opsOf(wSk));
    expect(await h.gk.sendRoster()).toEqual({ sent: [], held: [], failed: [] });
    expect(h.published).toHaveLength(0);
  });
});
