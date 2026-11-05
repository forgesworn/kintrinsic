// The guardian app's guardian-keys actions, tested against the contract
// (spec/contract.md, *Guardian roots and working keys (v1)*): statement
// shapes, who signs and who seals each one, the versioning gate, presence
// confirmation, the PIN and the roster. Wards are stood in for by their own
// secret keys, which open each wrap exactly as a ward would.

import { beforeEach, describe, expect, it } from "vitest";
import { finalizeEvent, generateSecretKey, getPublicKey, nip19, nip44, verifyEvent, type NostrEvent } from "nostr-tools";
import { fitRoster, GuardianKeys, MAX_ROSTER_BYTES, unwrapToWorkingKey, type KeysChild, type KeysDevice } from "./guardianKeys";
import { emptyGuardianKeys, PIN_RESEND_LIMIT, type GuardianKeysState } from "./state";
import { guardianPinHash } from "./pin";
import { familyRosterTemplate, parseStatement, CHARTER_PRESENCE_CONFIRM, CHARTER_FAMILY_ROSTER, type SignedEvent } from "./sdk";
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
  setRoot: (ops: GuardianOps | null) => void;
  setNow: (t: number) => void;
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
  let clock = T;
  const published: Harness["published"] = [];
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
    now: () => clock,
    defaultRelays: ["wss://default.example"],
  });
  return {
    gk,
    keys: () => keys,
    children,
    published,
    setRoot: (o) => (root = o),
    setNow: (t) => (clock = t),
  };
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
    appVersionCode: platform === "android" ? 55 : 804,
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

  it("refuses to re-approve with a different identity once a root is set (a move, not a re-approval)", async () => {
    const h = harness([]);
    await connectSignetRoot(h);
    h.setRoot(opsOf(r2Sk));
    await expect(connectSignetRoot(h)).rejects.toThrow(/different guardian identity/);
    expect(h.keys().root?.pubkey).toBe(R);
    h.setRoot(opsOf(rSk));
    await expect(connectSignetRoot(h)).resolves.toMatchObject({ pubkey: R });
  });

  it("drops an approval that names a different working key (a restored key backup)", async () => {
    const h = harness([]);
    await connectSignetRoot(h);
    expect(h.gk.dropStaleApproval()).toBe(false);
    // R's approval of some other W, as a backup made on another phone would hold.
    const otherW = getPublicKey(generateSecretKey());
    const elsewhere = finalizeEvent(
      {
        kind: 31120,
        created_at: T,
        tags: [["d", `kintrinsic/device/${otherW}`], ["p", otherW], ["t", "charter-device"]],
        content: JSON.stringify({ v: 1, issuedAt: T }),
      },
      rSk,
    );
    h.keys().root!.approval = elsewhere as SignedEvent;
    expect(h.gk.dropStaleApproval()).toBe(true);
    expect(h.keys().root).toBeUndefined();
  });

  it("forgets the root when the signer is disconnected", async () => {
    const h = harness([]);
    await connectSignetRoot(h);
    h.gk.forgetRoot();
    expect(h.keys().root).toBeUndefined();
    expect(h.gk.rootPubkey()).toBe(W);
  });

  it("after a disconnect, only an identity the devices list (or, with none reporting, the forgotten one) approves again (L2)", async () => {
    const silent = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [device(PHONE, "android")] }]);
    await connectSignetRoot(silent);
    silent.gk.forgetRoot();
    expect(silent.keys().forgottenRoot).toBe(R);
    silent.setRoot(opsOf(r2Sk));
    await expect(connectSignetRoot(silent)).rejects.toThrow(/different guardian identity/);
    silent.setRoot(opsOf(rSk));
    await expect(connectSignetRoot(silent)).resolves.toMatchObject({ pubkey: R });
    expect(silent.keys().forgottenRoot).toBeUndefined();

    const reporting = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] }]);
    await connectSignetRoot(reporting);
    reporting.gk.forgetRoot();
    reporting.setRoot(opsOf(r2Sk));
    await expect(connectSignetRoot(reporting)).rejects.toThrow(/different guardian identity/);
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
    child.devices[1] = device(LAPTOP, "linux", { statusSeen: true, appVersionCode: 803 });
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
      // The local key pinned as a root, beside R (connecting a root the
      // wards don't list is refused: batch 3b review L2).
      devices: [
        {
          ...readyWard(PHONE, "android"),
          guardians: { roots: [{ root: W, keys: [] }, { root: R, keys: [] }], pending: [], tombstones: 0, relays: [] },
        },
      ],
    };
    const h = harness([child]);
    await connectSignetRoot(h);
    expect(h.gk.keyFor(PHONE)).toBe("working");
  });

  it("has no working-key PAIR_OFFER: scan-to-pair is offered by the root", () => {
    const h = harness([]);
    expect("pairOffer" in h.gk).toBe(false);
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

  it("leaving is refused while any ward of this phone's hasn't reported, naming it (M3)", async () => {
    child.devices[1] = device(LAPTOP, "linux", { statusSeen: true });
    await expect(h.gk.leaveFamily()).rejects.toThrow(/Sam's laptop hasn't reported its guardians/);
    expect(h.published).toHaveLength(0);
    // A device known only from the other parent's roster, never heard from, isn't counted.
    child.devices[1] = device(LAPTOP, "linux", { fromRoster: true });
    await expect(h.gk.leaveFamily()).resolves.toMatchObject({ sent: expect.arrayContaining([PHONE, LAPTOP]) });
  });

  it("a removal of this phone's own identity can't be confirmed here (L1)", async () => {
    const code = `charter://presence?m=${PHONE}&t=${"ab".repeat(16)}&d=kintrinsic/unguardian/${R}&s=${"ef".repeat(32)}`;
    await expect(h.gk.confirmPresence(code)).rejects.toThrow(/request to remove you/);
    expect(h.published).toHaveLength(0);
  });

  it("removing the other parent sends only the removal: nothing is re-signed first (D2)", async () => {
    await h.gk.removeCoGuardian(R2);
    expect(delivered()).toHaveLength(child.devices.length);
    for (const o of delivered()) {
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "removal", d: `kintrinsic/unguardian/${R2}`, p: R2, signer: R });
    }
    await expect(h.gk.removeCoGuardian(R)).rejects.toThrow(/leave the family/);
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

  it("withdrawing another install: signed by R, sealed by this W", async () => {
    const W2 = getPublicKey(generateSecretKey());
    child.devices[0].guardians!.roots[0].keys.push(W2);
    await h.gk.withdrawInstall(W2);
    for (const o of delivered()) {
      const st = parseStatement(o.inner);
      expect(st.ok && st.statement).toMatchObject({ type: "withdrawal", d: `kintrinsic/revoke/${W2}`, p: W2, signer: R });
      expect(o.sealAuthor).toBe(o.to === PHONE ? W : R);
    }
  });

  it("withdrawing this install's own W is sealed by R itself", async () => {
    expect(h.gk.keyFor(PHONE)).toBe("working");
    await h.gk.withdrawInstall(W);
    for (const o of delivered()) expect(o.sealAuthor).toBe(R);
  });

  it("withdrawing this install's own W without R connected sends nothing", async () => {
    h.setRoot(null);
    await expect(h.gk.withdrawInstall(W)).rejects.toThrow(/Connect/);
    expect(h.published).toHaveLength(0);
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
    // Not before the inviting parent's guardian code has been scanned.
    await expect(h.gk.confirmPresence(code)).rejects.toThrow(/Scan the guardian code of the parent who invited you/);
    expect(h.published).toHaveLength(0);
    h.gk.anchorRoot(nip19.npubEncode(R2));
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
    // Only the install that started a removal can confirm it (batch 3b review L1).
    const code = `charter://presence?m=${PHONE}&t=${token}&d=kintrinsic/unguardian/${R2}&s=${stmt}`;
    await expect(h.gk.confirmPresence(code)).rejects.toThrow(/wasn't started from this phone/);
    await h.gk.removeCoGuardian(R2);
    const before = h.published.length;
    await h.gk.confirmPresence(code);
    const { inner } = open(h.published[before].wrap, phoneSk);
    expect(JSON.parse(inner.content)).toEqual({ v: 1, token, statement: stmt });
    expect(h.published[before].relays.sort()).toEqual([
      "wss://a.example",
      "wss://b.example",
      "wss://default.example",
      "wss://ward.example",
    ]);
  });

  it("this phone's own key can't confirm for devices that pin another identity", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] }], opsOf(wSk));
    await expect(
      h.gk.confirmPresence(`charter://presence?m=${PHONE}&t=${token}&d=kintrinsic/unguardian/${R2}&s=${stmt}`),
    ).rejects.toThrow(/use another guardian identity/);
    expect(h.published).toHaveLength(0);
  });
});

describe("root move", () => {
  it("R signs the move, R2 approves W, and the confirmation carries R2's approval", async () => {
    const child: KeysChild = { id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] };
    const h = harness([child]);
    await connectSignetRoot(h);
    await h.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "bunker://r2", via: "heartwood" });
    expect(h.published).toHaveLength(1);
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

  it("is refused while a ward isn't ready", async () => {
    const child: KeysChild = { id: "c1", name: "Sam", colour: "#000", devices: [device(PHONE, "android")] };
    const h = harness([child]);
    await connectSignetRoot(h);
    // The device has never reported: it hasn't checked in, rather than needing an update (N-L1).
    await expect(h.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "x", via: "signet" })).rejects.toThrow(
      /Sam's phone hasn't checked in with this phone yet/,
    );
    expect(h.published).toHaveLength(0);
  });

  describe("half-way through (H1)", () => {
    const LAPTOP_SK = laptopSk;
    let h: Harness;
    let child: KeysChild;
    const movedView = { roots: [{ root: R2, keys: [W] }], pending: [], tombstones: 0, relays: [] };

    beforeEach(async () => {
      child = { id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android"), readyWard(LAPTOP, "linux")] };
      h = harness([child]);
      await connectSignetRoot(h);
      await h.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "bunker://r2", via: "signet" });
      h.published.length = 0;
      // The phone confirmed the move: it pins R2 with W re-bound to R2. The laptop hasn't.
      child.devices[0].guardians = movedView;
      const moveD = `kintrinsic/root/${R2}`;
      child.devices[1].guardians = { ...child.devices[1].guardians!, pending: [{ d: moveD, kind: "move", expiresAt: T + 1000 }] };
    });

    it("each ward gets the key its own STATUS trusts: W for both, bound to R2 on the phone", () => {
      expect(h.gk.keyFor(PHONE)).toBe("working");
      expect(h.gk.keyFor(LAPTOP)).toBe("working");
      expect(h.gk.settleRootMove()).toBeNull();
      expect(h.keys().nextRoot?.pubkey).toBe(R2);
    });

    it("the approval resend sends R2's approval to the moved phone only when it lacks W, never R's", async () => {
      child.devices[0].guardians = { roots: [{ root: R2, keys: [] }], pending: [], tombstones: 0, relays: [] };
      await h.gk.syncApprovals({ force: true });
      expect(h.published.map((p) => recipient(p.wrap))).toEqual([PHONE]);
      const ap = parseStatement(open(h.published[0].wrap, phoneSk).inner);
      expect(ap.ok && ap.statement).toMatchObject({ type: "approval", signer: R2, key: W });
      child.devices[0].guardians = movedView;
      h.published.length = 0;
      await h.gk.syncApprovals({ force: true });
      expect(h.published).toHaveLength(0);
    });

    it("refuses root statements R could not deliver to the moved phone", async () => {
      await expect(h.gk.inviteCoGuardian(getPublicKey(generateSecretKey()))).rejects.toThrow(/already uses your new guardian identity/);
      await expect(h.gk.withdrawInstall(getPublicKey(generateSecretKey()))).rejects.toThrow(/Finish the move/);
      await expect(h.gk.removeCoGuardian(getPublicKey(generateSecretKey()))).rejects.toThrow(/Finish the move/);
      await expect(
        h.gk.moveRoot({ ops: opsOf(generateSecretKey()), bunkerUri: "bunker://r3", via: "signet" }),
      ).rejects.toThrow(/Finish that move first/);
      expect(h.published).toHaveLength(0);
    });

    it("says the move has lapsed and which devices switched, and points at finishing it (M1)", async () => {
      expect(h.gk.moveProgress()).toMatchObject({ to: R2, lapsed: false, moved: ["Sam's phone"] });
      h.setNow(T + 1000);
      expect(h.gk.moveProgress()).toMatchObject({ lapsed: true, moved: ["Sam's phone"] });
      await expect(h.gk.removeCoGuardian(getPublicKey(generateSecretKey()))).rejects.toThrow(/Finish my identity move/);
    });

    it("a cancel or objection R signs isn't reported as sent to the moved phone (L5)", async () => {
      const report = await h.gk.cancelPending(`kintrinsic/root/${R2}`);
      expect(report.sent).toEqual([LAPTOP]);
      expect(report.failed).toEqual([{ to: PHONE, error: "Sam's phone already uses your new guardian identity." }]);
      expect(h.published.map((p) => recipient(p.wrap))).toEqual([LAPTOP]);
    });

    it("doesn't settle while a ward hasn't reported (L7)", () => {
      child.devices[1] = device(LAPTOP, "linux", { statusSeen: true });
      expect(h.gk.settleRootMove()).toBeNull();
      expect(h.keys().root?.pubkey).toBe(R);
    });

    it("settles once every ward pins R2 and not R", () => {
      child.devices[1].guardians = movedView;
      expect(h.gk.settleRootMove()).toBe("bunker://r2");
      expect(h.keys().root?.pubkey).toBe(R2);
      expect(h.keys().nextRoot).toBeUndefined();
    });

    it("a lapsed move with a ward already on R2 keeps R2, so that ward isn't stranded; restarting it finishes", async () => {
      h.setNow(T + 1000); // the laptop's pending move has reached its expiresAt
      expect(h.gk.settleRootMove()).toBeNull();
      expect(h.keys().nextRoot?.pubkey).toBe(R2);
      expect(h.gk.keyFor(PHONE)).toBe("working");
      // Restarting the move to the same identity is allowed.
      await h.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "bunker://r2", via: "signet" });
      const toLaptop = h.published.filter((p) => recipient(p.wrap) === LAPTOP);
      expect(toLaptop).toHaveLength(1);
      const st = parseStatement(open(toLaptop[0].wrap, LAPTOP_SK).inner);
      expect(st.ok && st.statement).toMatchObject({ type: "move", signer: R });
    });
  });

  describe("cleared when it lapses with no ward moved (H1)", () => {
    let h: Harness;
    let child: KeysChild;

    beforeEach(async () => {
      child = { id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] };
      h = harness([child]);
      await connectSignetRoot(h);
      await h.gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "bunker://r2", via: "signet" });
    });

    it("by expiry, read from the wards' STATUS pending", () => {
      const moveD = `kintrinsic/root/${R2}`;
      child.devices[0].guardians = { ...child.devices[0].guardians!, pending: [{ d: moveD, kind: "move", expiresAt: T + 50 }] };
      h.setNow(T + 49);
      expect(h.gk.settleRootMove()).toBeNull();
      expect(h.keys().nextRoot).toBeDefined();
      h.setNow(T + 50);
      expect(h.gk.settleRootMove()).toBeNull();
      expect(h.keys().nextRoot).toBeUndefined();
      expect(h.keys().root?.pubkey).toBe(R);
    });

    it("by expiry, from the move's own 7 days once no ward lists it", () => {
      h.setNow(T + 7 * 24 * 3600 - 1);
      h.gk.settleRootMove();
      expect(h.keys().nextRoot).toBeDefined();
      h.setNow(T + 7 * 24 * 3600);
      h.gk.settleRootMove();
      expect(h.keys().nextRoot).toBeUndefined();
    });

    it("by this install cancelling it", async () => {
      await h.gk.cancelPending(`kintrinsic/root/${R2}`);
      expect(h.keys().nextRoot).toBeUndefined();
      expect(h.keys().root?.pubkey).toBe(R);
    });
  });
});

describe("objecting automatically to a removal of this parent (Q1)", () => {
  const removalOfMe = `kintrinsic/unguardian/${R}`;
  const ID = "ab".repeat(32);
  const pending = (over: { id?: string; expiresAt?: number } = {}) => [
    { d: removalOfMe, kind: "removal" as const, takesEffectAt: T + 72 * 3600, expiresAt: T + 7 * 86400, ...over },
  ];
  const view = (roots: { root: string; keys: string[] }[], p = pending({ id: ID })) => ({
    roots,
    pending: p,
    tombstones: 0,
    relays: ["wss://ward.example"],
  });

  it("objects once per removal per ward, signed and sealed by W, naming the other root", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android", [{ root: R2, keys: [] }])] }]);
    await connectSignetRoot(h);
    h.published.length = 0;
    const g = view([{ root: R, keys: [W] }, { root: R2, keys: [] }]);
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 10, guardians: g });
    expect(h.published).toHaveLength(1);
    const { sealAuthor, inner } = open(h.published[0].wrap, phoneSk);
    expect(sealAuthor).toBe(W);
    const st = parseStatement(inner);
    expect(st.ok && st.statement).toMatchObject({ type: "objection", d: `kintrinsic/object/${removalOfMe}`, p: R2, signer: W });
    expect(h.keys().objected).toEqual({ [PHONE]: [ID] });
    // The same removal again, in this or a later STATUS: no second objection.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 20, guardians: g });
    expect(h.published).toHaveLength(1);
    // Re-issued after the block (a new id): objected to afresh.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 30, guardians: view(g.roots, pending({ id: "cd".repeat(32) })) });
    expect(h.published).toHaveLength(2);
    expect(h.keys().objected).toEqual({ [PHONE]: ["cd".repeat(32)] });
    // Gone from the ward's report: forgotten.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 40, guardians: view(g.roots, []) });
    expect(h.keys().objected).toEqual({});
  });

  it("doesn't object again when polling re-delivers an older STATUS after the removal was dropped", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android", [{ root: R2, keys: [] }])] }]);
    await connectSignetRoot(h);
    h.published.length = 0;
    const roots = [{ root: R, keys: [W] }, { root: R2, keys: [] }];
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 100, guardians: view(roots) });
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 200, guardians: view(roots, []) });
    expect(h.keys().objected).toEqual({});
    // The relay hands back both again on the next poll.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 100, guardians: view(roots) });
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 200, guardians: view(roots, []) });
    expect(h.published).toHaveLength(1);
    expect(h.keys().objected).toEqual({});
  });

  it("keys a removal by its d where the ward reports no id", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android", [{ root: R2, keys: [] }])] }]);
    await connectSignetRoot(h);
    h.published.length = 0;
    const g = view([{ root: R, keys: [W] }, { root: R2, keys: [] }], pending());
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 10, guardians: g });
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 11, guardians: g });
    expect(h.published).toHaveLength(1);
    expect(h.keys().objected).toEqual({ [PHONE]: [removalOfMe] });
  });

  it("sends nothing when the remover is ambiguous, W isn't listed there, or the removal has expired", async () => {
    const R3 = getPublicKey(generateSecretKey());
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android", [{ root: R2, keys: [] }])] }]);
    await connectSignetRoot(h);
    h.published.length = 0;
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 10, guardians: view([{ root: R, keys: [W] }, { root: R2, keys: [] }, { root: R3, keys: [] }]) });
    // W not bound here yet: the root would have to sign, which needs a prompt.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 11, guardians: view([{ root: R, keys: [] }, { root: R2, keys: [] }]) });
    await h.gk.noteWardStatus({
      machine: PHONE,
      ts: T + 12,
      guardians: view([{ root: R, keys: [W] }, { root: R2, keys: [] }], pending({ id: ID, expiresAt: T - 1 })),
    });
    expect(h.published).toHaveLength(0);
    expect(h.keys().objected).toEqual({});
  });

  it("a phone-held root objects with its own key", async () => {
    const removalOfW = `kintrinsic/unguardian/${W}`;
    const ward = device(PHONE, "android", { statusSeen: true, appVersionCode: 55 });
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [ward] }], opsOf(wSk));
    const g = {
      roots: [{ root: W, keys: [] }, { root: R2, keys: [] }],
      pending: [{ d: removalOfW, kind: "removal" as const, expiresAt: T + 100 }],
      tombstones: 0,
      relays: [],
    };
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 10, guardians: g });
    expect(h.published).toHaveLength(1);
    const { sealAuthor, inner } = open(h.published[0].wrap, phoneSk);
    expect(sealAuthor).toBe(W);
    const st = parseStatement(inner);
    expect(st.ok && st.statement).toMatchObject({ type: "objection", d: `kintrinsic/object/${removalOfW}`, p: R2, signer: W });
  });

  it("doesn't object to a removal this install sent itself", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android", [{ root: R2, keys: [] }])] }]);
    await connectSignetRoot(h);
    const left = await h.gk.leaveFamily();
    h.published.length = 0;
    await h.gk.noteWardStatus({
      machine: PHONE,
      ts: T + 10,
      guardians: view([{ root: R, keys: [W] }, { root: R2, keys: [] }], pending({ id: left.event.id })),
    });
    expect(h.published).toHaveLength(0);
  });
});

describe("guardian PIN", () => {
  it("sends a gift-wrapped PIN-only RELAYS to ready Android wards only", async () => {
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
      devices: [device(old, "android", { name: "Old tablet", statusSeen: true, appVersionCode: 54 })],
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
    // PIN-only: no relay list, so it can't change or revert the ward's relays (D4).
    expect("relays" in body).toBe(false);
    expect(await guardianPinHash("123456", body.guardianPin.salt)).toBe(body.guardianPin.hash);
    expect(h.keys().pins).toEqual({ [PHONE]: { state: "sending", record: body.guardianPin, sentAt: T } });
    // Only the phones named, when the screen names them (L4).
    h.published.length = 0;
    const named = await h.gk.setGuardianPin("123456", { machines: [old] });
    expect(named.sent).toEqual([]);
    expect(named.failed.map((f) => f.to)).toEqual([old]);
    expect(h.published).toHaveLength(0);
    expect(JSON.stringify(h.keys())).not.toContain("123456");
  }, 30_000);

  it("stays sending until a STATUS shows pinSet, resending the same record once per newer STATUS", async () => {
    const ward = readyWard(PHONE, "android");
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [ward] }]);
    await connectSignetRoot(h);
    await h.gk.setGuardianPin("123456");
    const first = JSON.parse(open(h.published[0].wrap, phoneSk).inner.content).guardianPin;
    h.published.length = 0;
    const g = (pinSet?: boolean) => ({ ...ward.guardians!, ...(pinSet !== undefined ? { pinSet } : {}) });

    // A STATUS without pinSet: one resend of the same record, newer created_at.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 10, guardians: g(false) });
    expect(h.published).toHaveLength(1);
    const again = open(h.published[0].wrap, phoneSk).inner;
    expect(JSON.parse(again.content)).toEqual({ machine: PHONE, guardianPin: first });
    expect(again.created_at).toBe(T + 1);
    // The same STATUS polled again, or an older one: nothing more.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 10, guardians: g(false) });
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 5, guardians: g() });
    expect(h.published).toHaveLength(1);
    // A newer one that doesn't say: once more.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 20, guardians: g() });
    expect(h.published).toHaveLength(2);
    expect(h.keys().pins[PHONE].state).toBe("sending");

    // The ward says it holds one: set, the record dropped, nothing sent.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 30, guardians: g(true) });
    expect(h.published).toHaveLength(2);
    expect(h.keys().pins[PHONE]).toEqual({ state: "set", sentAt: T + 2, statusTs: T + 30 });
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 40, guardians: g(false) });
    expect(h.published).toHaveLength(2);
  }, 30_000);

  it("stops resending after PIN_RESEND_LIMIT reports without pinSet, until the guardian tries again", async () => {
    const ward = readyWard(PHONE, "android");
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [ward] }]);
    await connectSignetRoot(h);
    await h.gk.setGuardianPin("123456");
    h.published.length = 0;
    const g = { ...ward.guardians!, pinSet: false };
    for (let i = 1; i <= PIN_RESEND_LIMIT; i++) await h.gk.noteWardStatus({ machine: PHONE, ts: T + i * 10, guardians: g });
    expect(PIN_RESEND_LIMIT).toBe(20);
    expect(h.published).toHaveLength(20);
    expect(h.keys().pins[PHONE].state).toBe("sending");
    // The 21st: nothing sent, and the guardian is told.
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 1000, guardians: g });
    expect(h.published).toHaveLength(20);
    expect(h.keys().pins[PHONE]).toEqual({ state: "unconfirmed", sentAt: T + 20, statusTs: T + 1000 });
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 2000, guardians: g });
    expect(h.published).toHaveLength(20);
    // Trying again starts over.
    await h.gk.setGuardianPin("123456");
    expect(h.published).toHaveLength(21);
    expect(h.keys().pins[PHONE].state).toBe("sending");
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 3000, guardians: g });
    expect(h.published).toHaveLength(22);
    expect(h.keys().pins[PHONE].resends).toBe(1);
  }, 60_000);

  it("never sends a PIN to a ward below the versioning gate, one that never reports pinSet", async () => {
    const old = device(PHONE, "android", {
      statusSeen: true,
      appVersionCode: 54,
      guardians: { roots: [{ root: R, keys: [W] }], pending: [], tombstones: 0, relays: [] },
    });
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [old] }]);
    await connectSignetRoot(h);
    h.published.length = 0;
    const report = await h.gk.setGuardianPin("123456");
    expect(report.sent).toEqual([]);
    expect(report.failed.map((f) => f.to)).toEqual([PHONE]);
    for (let i = 1; i <= 3; i++) await h.gk.noteWardStatus({ machine: PHONE, ts: T + i * 10, guardians: old.guardians });
    expect(h.published).toHaveLength(0);
    expect(h.keys().pins[PHONE]).toBeUndefined();
  }, 30_000);

  it("doesn't resend unattended where the root would have to sign (an approval prompt)", async () => {
    // The ward doesn't list W yet: its events are signed by R over My Signet.
    const ward = device(PHONE, "android", {
      statusSeen: true,
      appVersionCode: 55,
      guardians: { roots: [{ root: R, keys: [] }], pending: [], tombstones: 0, relays: [] },
    });
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [ward] }]);
    await connectSignetRoot(h);
    h.published.length = 0;
    await h.gk.setGuardianPin("123456");
    expect(open(h.published[0].wrap, phoneSk).inner.pubkey).toBe(R);
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 10, guardians: { ...ward.guardians!, pinSet: false } });
    expect(h.published).toHaveLength(1);
    expect(h.keys().pins[PHONE].state).toBe("sending");
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
    // R2's working key is reported by the ward, but R2 isn't trusted yet: nothing goes to it.
    expect((await h.gk.sendRoster()).sent).toEqual([]);
    h.gk.anchorRoot(R2);
    h.published.length = 0;
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
    // Not until the other install trusts R (its scanned guardian code).
    expect(other.receiveRoster(opened)).toEqual({ ok: false, reason: "root_not_trusted" });
    other.anchorRoot(`nostr:${nip19.npubEncode(R)}`);
    expect(other.receiveRoster(opened)).toEqual({ ok: true, updated: [subject] });
    expect(other.unconfirmedRosterDevices()).toEqual([{ subject, machine: PHONE, name: "Sam's phone", platform: "android" }]);
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
    expect(await h.gk.sendRoster()).toEqual({ events: [], sent: [], held: [], failed: [], dropped: [] });
    expect(h.published).toHaveLength(0);
  });
});

describe("anchoring the other parent's root (batch 3 re-review N-H1)", () => {
  it("takes a scanned or pasted guardian code, and refuses anything else", async () => {
    const h = harness([]);
    await connectSignetRoot(h);
    expect(() => h.gk.anchorRoot("hello")).toThrow(/isn't a guardian code/);
    expect(() => h.gk.anchorRoot("npub1qqqqqqqq")).toThrow(/isn't a guardian code/);
    expect(() => h.gk.anchorRoot(nip19.npubEncode(R))).toThrow(/your own guardian code/);
    expect(() => h.gk.anchorRoot(W)).toThrow(/this phone's own key/);
    expect(h.keys().anchoredRoots).toEqual({});
    expect(h.gk.trustedRoots()).toEqual([R]);
    expect(h.gk.anchorRoot(` nostr:${nip19.npubEncode(R2)} `)).toBe(R2);
    expect(h.keys().anchoredRoots[R2]).toEqual({ anchoredAt: T, how: "scanned" });
    expect(h.gk.trustedRoots()).toEqual([R, R2].sort());
  });

  it("an invitation anchors the invited root, so the two installs share the family", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] }]);
    await connectSignetRoot(h);
    await h.gk.inviteCoGuardian(R2, { label: "Dad" });
    expect(h.keys().anchoredRoots[R2]).toEqual({ anchoredAt: T, how: "invited", label: "Dad" });
    expect(h.gk.trustedRoots()).toContain(R2);
  });

  it("a roster whose approval an untrusted root signed is refused for good", async () => {
    const h = harness([]);
    await connectSignetRoot(h);
    const xSk = generateSecretKey();
    const wxSk = generateSecretKey();
    // An attacker's own install: its root approves its own W, which it can always do.
    let xKeys = emptyGuardianKeys();
    const X = new GuardianKeys({
      read: () => ({ keys: xKeys, children: [] }),
      write: (f) => {
        xKeys = f(xKeys);
      },
      workingKey: () => wxSk,
      root: () => opsOf(xSk),
      publish: async () => {},
      now: () => T,
      defaultRelays: [],
    });
    await X.approveThisInstall({ via: "signet", label: "X" });
    const approval = xKeys.root!.approval;
    const inner = finalizeEvent(
      {
        kind: CHARTER_FAMILY_ROSTER,
        created_at: T,
        tags: [],
        content: JSON.stringify({ v: 1, children: [{ subject: "cd".repeat(32), issuedAt: T, name: "Fake kid", colour: "#f00", devices: [] }], approval }),
      },
      wxSk,
    ) as SignedEvent;
    expect(h.gk.receiveRoster({ sealAuthor: getPublicKey(wxSk), inner })).toEqual({ ok: false, reason: "root_not_trusted" });
    expect(h.keys().roster.children).toEqual([]);
  });

  it("presence wards expire a week after the confirmation, and never widen the roster's recipients", async () => {
    const h = harness([]);
    await connectSignetRoot(h);
    h.gk.anchorRoot(R2);
    const code = `charter://presence?m=${PHONE}&t=${"0".repeat(32)}&d=${encodeURIComponent(`kintrinsic/guardian/${R}`)}&s=${"1".repeat(64)}`;
    await h.gk.confirmPresence(code);
    const W_X = getPublicKey(generateSecretKey());
    h.gk.notePresenceStatus({
      machine: PHONE,
      ts: T + 10,
      appVersionCode: 55,
      // A presence ward can claim anything, even keys under a trusted root.
      guardians: { roots: [{ root: R2, keys: [W_X] }], pending: [], tombstones: 0, relays: [] },
    });
    expect(h.gk.isPresenceWard(PHONE)).toBe(true);
    expect(h.gk.waitingPresenceWards().map((p) => p.machine)).toEqual([PHONE]);
    expect(h.gk.reportedRoots()).toEqual([]);
    h.setNow(T + 7 * 86400 - 1);
    expect(h.gk.isPresenceWard(PHONE)).toBe(true);
    h.setNow(T + 7 * 86400);
    expect(h.gk.isPresenceWard(PHONE)).toBe(false);
    expect(h.gk.waitingPresenceWards()).toEqual([]);
    h.gk.prunePresenceWards();
    expect(h.keys().presenceWards).toEqual({});
  });
});

describe("readiness and roster-only devices (batch 3 re-review N-L1)", () => {
  it("a roster-only device never heard from doesn't hold back inviting or W signing", () => {
    const child: KeysChild = {
      id: "c1",
      name: "Sam",
      colour: "#000",
      devices: [readyWard(PHONE, "android"), device(LAPTOP, "linux", { fromRoster: true })],
    };
    const h = harness([child]);
    expect(h.gk.familyReadiness().ready).toBe(true);
    expect(h.gk.childReadiness("c1").wards.map((w) => w.machine)).toEqual([PHONE]);
    // Once it has reported here, it counts again.
    child.devices[1] = { ...child.devices[1], statusSeen: true, appVersionCode: 700 };
    expect(h.gk.familyReadiness().ready).toBe(false);
  });

  it("names a silent device as not checked in, and an old one as needing an update", async () => {
    const child: KeysChild = {
      id: "c1",
      name: "Sam",
      colour: "#000",
      devices: [device(PHONE, "android"), device(LAPTOP, "linux", { statusSeen: true, appVersionCode: 700 })],
    };
    const h = harness([child]);
    await connectSignetRoot(h);
    await expect(h.gk.inviteCoGuardian(R2)).rejects.toThrow(
      "Sam's phone hasn't checked in with this phone yet, and Sam's laptop needs a Kintrinsic update. Then invite another parent.",
    );
  });
});

describe("a phone-held root mints a working key (decision D5)", () => {
  const pSk = generateSecretKey();
  const P = getPublicKey(pSk);
  const token = "0123456789abcdef0123456789abcdef";
  const stmt = "ef".repeat(32);

  function phoneHarness(roots: { root: string; keys: string[] }[] = [{ root: P, keys: [] }, { root: R2, keys: [] }]) {
    let keys = emptyGuardianKeys();
    let minted: Uint8Array | null = null;
    let mints = 0;
    const published: { relays: string[]; wrap: NostrEvent }[] = [];
    const ward = device(PHONE, "android", {
      statusSeen: true,
      appVersionCode: 55,
      guardians: { roots, pending: [], tombstones: 0, relays: [] },
    });
    const children: KeysChild[] = [{ id: "c1", name: "Sam", colour: "#000", devices: [ward] }];
    const gk = new GuardianKeys({
      read: () => ({ keys, children }),
      write: (f) => {
        keys = f(keys);
      },
      workingKey: () => minted ?? pSk,
      localKey: () => pSk,
      mintWorkingKey: () => {
        mints++;
        minted = generateSecretKey();
        return minted;
      },
      root: () => opsOf(pSk),
      publish: async (relays, wrap) => {
        published.push({ relays, wrap });
      },
      now: () => T,
      defaultRelays: ["wss://default.example"],
    });
    return {
      gk,
      keys: () => keys,
      published,
      ward,
      minted: () => minted!,
      mints: () => mints,
      list: (g: KeysDevice["guardians"]) => {
        ward.guardians = g;
      },
    };
  }

  it("removing the other parent mints W, sends R's approval of it, then the removal", async () => {
    const h = phoneHarness();
    const report = await h.gk.removeCoGuardian(R2);
    const w = getPublicKey(h.minted());
    expect(w).not.toBe(P);
    expect(h.keys().root).toMatchObject({ pubkey: P, via: "phone" });
    const approval = parseStatement(h.keys().root!.approval);
    expect(approval.ok && approval.statement).toMatchObject({ type: "approval", signer: P, key: w });
    expect(h.published).toHaveLength(2);
    // The approval first, sealed by the W it names.
    const first = open(h.published[0].wrap, phoneSk);
    expect(first.sealAuthor).toBe(w);
    expect(first.inner.id).toBe(h.keys().root!.approval.id);
    // The removal is R's; the ward doesn't hold W yet, so R seals it too.
    const second = open(h.published[1].wrap, phoneSk);
    expect(second.inner.id).toBe(report.event.id);
    expect(second.inner.pubkey).toBe(P);
    expect(second.sealAuthor).toBe(P);
    expect(h.gk.rootPubkey()).toBe(P);
    expect(h.gk.workingPubkey()).toBe(w);
  });

  it("R keeps signing the rules day to day once W is listed", async () => {
    const h = phoneHarness();
    await h.gk.ensureRoot("test");
    const w = getPublicKey(h.minted());
    h.list({ roots: [{ root: P, keys: [w] }], pending: [], tombstones: 0, relays: [] });
    expect(h.gk.keyFor(PHONE)).toBe("root");
    expect(h.gk.opsFor(PHONE).pubkey).toBe(P);
  });

  it("confirms the removal in person with W once the device lists it, and not before", async () => {
    const h = phoneHarness();
    await h.gk.removeCoGuardian(R2);
    const d = `kintrinsic/unguardian/${R2}`;
    const code = `charter://presence?m=${PHONE}&t=${token}&d=${d}&s=${stmt}`;
    await expect(h.gk.confirmPresence(code)).rejects.toThrow(/hasn't taken this phone's day-to-day key yet/);
    const w = getPublicKey(h.minted());
    h.list({ roots: [{ root: P, keys: [w] }, { root: R2, keys: [] }], pending: [], tombstones: 0, relays: [] });
    const before = h.published.length;
    await h.gk.confirmPresence(code);
    const { sealAuthor, inner } = open(h.published[before].wrap, phoneSk);
    expect(inner.kind).toBe(CHARTER_PRESENCE_CONFIRM);
    expect(inner.pubkey).toBe(w);
    expect(sealAuthor).toBe(w);
  });

  it("seals the statements R signs with W where the device lists W", async () => {
    const h = phoneHarness();
    await h.gk.ensureRoot("test");
    const w = getPublicKey(h.minted());
    h.list({ roots: [{ root: P, keys: [w] }, { root: R2, keys: [] }], pending: [], tombstones: 0, relays: [] });
    const before = h.published.length;
    const r = await h.gk.removeCoGuardian(R2);
    const { sealAuthor, inner } = open(h.published[before].wrap, phoneSk);
    expect(inner.id).toBe(r.event.id);
    expect(inner.pubkey).toBe(P);
    expect(sealAuthor).toBe(w);
  });

  it("re-uses W after the root was forgotten, and never approves the root as its own working key", async () => {
    const h = phoneHarness();
    await h.gk.ensureRoot("test");
    h.gk.forgetRoot();
    await h.gk.ensureRoot("test");
    expect(h.mints()).toBe(1);

    let keys = emptyGuardianKeys();
    const self = new GuardianKeys({
      read: () => ({ keys, children: [] }),
      write: (f) => {
        keys = f(keys);
      },
      workingKey: () => pSk,
      localKey: () => pSk,
      mintWorkingKey: () => pSk,
      root: () => opsOf(pSk),
      publish: async () => {},
      now: () => T,
      defaultRelays: [],
    });
    await expect(self.ensureRoot("test")).rejects.toThrow(/can't approve itself/);
    expect(keys.root).toBeUndefined();
  });

  it("objects with W where the device lists it, and with R where it doesn't yet", async () => {
    const removal = `kintrinsic/unguardian/${P}`;
    const g = (keys: string[]) => ({
      roots: [{ root: P, keys }, { root: R2, keys: [] }],
      pending: [{ d: removal, id: "cd".repeat(32), kind: "removal" as const, expiresAt: T + 100 }],
      tombstones: 0,
      relays: [],
    });
    const h = phoneHarness();
    await h.gk.ensureRoot("test");
    const w = getPublicKey(h.minted());
    const before = h.published.length;
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 1, guardians: g([]) });
    expect(open(h.published[before].wrap, phoneSk).inner.pubkey).toBe(P);

    const h2 = phoneHarness();
    await h2.gk.ensureRoot("test");
    const w2 = getPublicKey(h2.minted());
    const before2 = h2.published.length;
    await h2.gk.noteWardStatus({ machine: PHONE, ts: T + 1, guardians: g([w2]) });
    const o = open(h2.published[before2].wrap, phoneSk);
    expect(o.inner.pubkey).toBe(w2);
    expect(o.sealAuthor).toBe(w2);
    expect(w).not.toBe(w2);
  });
});

describe("STATUS follow-ups: the remover's key and the clock counter", () => {
  it("objects naming the signer the device reports, even with several other parents", async () => {
    const R3 = getPublicKey(generateSecretKey());
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] }]);
    await connectSignetRoot(h);
    const removal = `kintrinsic/unguardian/${R}`;
    await h.gk.noteWardStatus({
      machine: PHONE,
      ts: T + 10,
      guardians: {
        roots: [{ root: R, keys: [W] }, { root: R2, keys: [] }, { root: R3, keys: [] }],
        pending: [{ d: removal, id: "ab".repeat(32), signer: R3, kind: "removal", expiresAt: T + 1000 }],
        tombstones: 0,
        relays: [],
      },
    });
    expect(h.published).toHaveLength(1);
    const st = parseStatement(open(h.published[0].wrap, phoneSk).inner);
    expect(st.ok && st.statement).toMatchObject({ type: "objection", p: R3, signer: W });
  });

  it("remembers when a device's clamp counter rises, not its first report", async () => {
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [readyWard(PHONE, "android")] }]);
    const g = (clamped: number) => ({ roots: [{ root: R, keys: [W] }], pending: [], tombstones: 0, relays: [], clamped });
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 1, guardians: g(3) });
    expect(h.keys().clamped[PHONE]).toEqual({ count: 3 });
    h.setNow(T + 50);
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 2, guardians: g(4) });
    expect(h.keys().clamped[PHONE]).toEqual({ count: 4, roseAt: T + 50 });
    h.setNow(T + 90);
    await h.gk.noteWardStatus({ machine: PHONE, ts: T + 3, guardians: g(4) });
    expect(h.keys().clamped[PHONE]).toEqual({ count: 4, roseAt: T + 50 });
  });
});

describe("after a restore from backup (decision D5)", () => {
  it("offers the lost phone's key while the devices list it, and a phone-held identity withdraws it itself", async () => {
    const LOST = getPublicKey(generateSecretKey());
    const ward = device(PHONE, "android", {
      statusSeen: true,
      appVersionCode: 55,
      guardians: { roots: [{ root: W, keys: [LOST] }], pending: [], tombstones: 0, relays: [] },
    });
    // The restored phone: its key is the identity (W here is the local key), no W minted yet.
    const h = harness([{ id: "c1", name: "Sam", colour: "#000", devices: [ward] }], opsOf(wSk));
    expect(h.gk.restoreFollowUp()).toBeNull();
    h.gk["deps"].write((k) => ({ ...k, restoredFrom: { at: T, workingKeys: [{ pubkey: LOST, label: "Old phone" }, { pubkey: W }] } }));
    expect(h.gk.restoreFollowUp()).toEqual({ at: T, lost: [{ pubkey: LOST, label: "Old phone" }] });
    const r = await h.gk.withdrawInstall(LOST);
    const { sealAuthor, inner } = open(h.published[0].wrap, phoneSk);
    expect(inner.id).toBe(r.event.id);
    expect(inner.pubkey).toBe(W);
    expect(sealAuthor).toBe(W);
    // Once the devices drop it, there is nothing left to offer.
    ward.guardians = { roots: [{ root: W, keys: [] }], pending: [], tombstones: 1, relays: [] };
    expect(h.gk.restoreFollowUp()!.lost).toEqual([]);
    h.gk.forgetRestore();
    expect(h.gk.restoreFollowUp()).toBeNull();
  });
});

describe("moving a phone-held root to My Signet (decision D5)", () => {
  it("R signs the move, R2 approves the minted W (never R), and the old key may go once every device follows R2", async () => {
    const pSk = generateSecretKey();
    const P = getPublicKey(pSk);
    let keys = emptyGuardianKeys();
    let minted: Uint8Array | null = null;
    const published: { relays: string[]; wrap: NostrEvent }[] = [];
    const ward = device(PHONE, "android", {
      statusSeen: true,
      appVersionCode: 55,
      guardians: { roots: [{ root: P, keys: [] }], pending: [], tombstones: 0, relays: [] },
    });
    const gk = new GuardianKeys({
      read: () => ({ keys, children: [{ id: "c1", name: "Sam", colour: "#000", devices: [ward] }] }),
      write: (f) => {
        keys = f(keys);
      },
      workingKey: () => minted ?? pSk,
      localKey: () => pSk,
      mintWorkingKey: () => (minted = generateSecretKey()),
      root: () => opsOf(pSk),
      publish: async (relays, wrap) => {
        published.push({ relays, wrap });
      },
      now: () => T,
      defaultRelays: [],
    });
    const report = await gk.moveRoot({ ops: opsOf(r2Sk), bunkerUri: "bunker://new", via: "signet" });
    const w = getPublicKey(minted!);
    const move = parseStatement(report.event);
    expect(move.ok && move.statement).toMatchObject({ type: "move", signer: P, key: R2 });
    const r2Approval = parseStatement(keys.nextRoot!.approval);
    expect(r2Approval.ok && r2Approval.statement).toMatchObject({ type: "approval", signer: R2, key: w });

    // The confirmation at the device carries R2's approval of W.
    const before = published.length;
    await gk.confirmPresence(`charter://presence?m=${PHONE}&t=${"0".repeat(32)}&d=${keys.nextRoot!.moveD}&s=${report.event.id}`);
    const conf = open(published[before].wrap, phoneSk);
    expect(conf.inner.pubkey).toBe(w);
    expect(JSON.parse(conf.inner.content).approval.id).toBe(keys.nextRoot!.approval.id);

    // Half-way: the moved device takes the rules from W, not the old key.
    ward.guardians = { roots: [{ root: R2, keys: [w] }], pending: [], tombstones: 0, relays: [] };
    expect(gk.keyFor(PHONE)).toBe("working");
    expect(gk.phoneKeyRetirable()).toBe(false); // the move isn't settled yet
    expect(gk.settleRootMove()).toBe("bunker://new");
    expect(keys.root).toMatchObject({ pubkey: R2, via: "signet" });
    expect(gk.phoneKeyRetirable()).toBe(true);
    // A device still pinning the old key keeps it.
    ward.guardians = { roots: [{ root: R2, keys: [w] }, { root: P, keys: [] }], pending: [], tombstones: 0, relays: [] };
    expect(gk.phoneKeyRetirable()).toBe(false);
  });
});

describe("the roster carries the rules in force (decision Q3)", () => {
  const clause = (sk: Uint8Array, kind: string, issuedAt: number, body: unknown = {}) =>
    finalizeEvent(
      { kind: 31113, created_at: issuedAt, tags: [["t", "charter-device"]], content: JSON.stringify({ v: 1, kind, issuedAt, body }) },
      sk,
    ) as SignedEvent;

  it("sends one clause per kind, and the other install keeps only verified ones from a current signer, newest per kind", async () => {
    const W2sk = generateSecretKey();
    const W2 = getPublicKey(W2sk);
    const guardians = { roots: [{ root: R, keys: [W] }, { root: R2, keys: [W2] }], pending: [], tombstones: 0, relays: [] };
    const subject = "aa".repeat(32);
    const child: KeysChild = { id: "c1", name: "Sam", colour: "#000", rosterId: subject, devices: [{ ...readyWard(PHONE, "android"), guardians }] };
    const h = harness([child]);
    await connectSignetRoot(h);
    h.gk.anchorRoot(R2);
    // Newest per kind: the older schedule is replaced, an older one after it is ignored.
    expect(h.gk.recordRules(subject, [{ event: clause(wSk, "schedule", T - 10) }, { event: clause(wSk, "budget", T - 5) }])).toEqual(["budget", "schedule"]);
    expect(h.gk.recordRules(subject, [{ event: clause(wSk, "schedule", T) }])).toEqual(["schedule"]);
    expect(h.gk.recordRules(subject, [{ event: clause(wSk, "schedule", T - 1) }])).toEqual([]);
    h.published.length = 0;
    await h.gk.sendRoster();
    const opened = unwrapToWorkingKey(h.published[0].wrap, W2sk)!;
    const sent = JSON.parse(opened.inner.content).children[0].rules as SignedEvent[];
    expect(sent.map((e) => JSON.parse(e.content).kind)).toEqual(["budget", "schedule"]);

    // Smuggle in a clause from a stranger and a tampered one: the sender's own
    // signature covers them, but each rule is checked on its own.
    const stranger = generateSecretKey();
    const tampered = { ...clause(wSk, "content", T), content: JSON.stringify({ v: 1, kind: "content", issuedAt: T, body: { x: 1 } }) };
    const content = JSON.parse(opened.inner.content);
    content.children[0].rules = [...sent, clause(stranger, "apps", T), tampered];
    const inner = finalizeEvent({ kind: opened.inner.kind, created_at: T, tags: opened.inner.tags, content: JSON.stringify(content) }, wSk) as SignedEvent;

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
    other.anchorRoot(R);
    // The other parent already holds a newer budget of their own.
    other.recordRules(subject, [{ event: clause(W2sk, "budget", T + 5) }]);
    expect(other.receiveRoster({ sealAuthor: W, inner })).toMatchObject({ ok: true });
    const held = keys2.rules[subject];
    expect(Object.keys(held).sort()).toEqual(["budget", "schedule"]);
    expect(held.schedule.id).toBe(sent[1].id);
    expect(held.budget.pubkey).toBe(W2);
    // And it shares them on in its own roster.
    expect(other.rulesFor(subject).map((r) => r.event.id)).toEqual([held.budget.id, held.schedule.id]);
  });

  it("keys rules by device: device rules travel apart, and a newer family-wide rule replaces older ones", async () => {
    const W2sk = generateSecretKey();
    const W2 = getPublicKey(W2sk);
    const guardians = { roots: [{ root: R, keys: [W] }, { root: R2, keys: [W2] }], pending: [], tombstones: 0, relays: [] };
    const subject = "aa".repeat(32);
    const child: KeysChild = { id: "c1", name: "Sam", colour: "#000", rosterId: subject, devices: [{ ...readyWard(PHONE, "android"), guardians }] };
    const h = harness([child]);
    await connectSignetRoot(h);
    h.gk.anchorRoot(R2);
    const old = clause(wSk, "schedule", T - 20);
    const onPhone = clause(wSk, "schedule", T - 10, { phone: 1 });
    const onLaptop = clause(wSk, "schedule", T - 10, { laptop: 1 });
    expect(h.gk.recordRules(subject, [{ event: old }, { event: onPhone, machine: PHONE }, { event: onLaptop, machine: LAPTOP }])).toEqual(
      ["schedule", `schedule@${LAPTOP}`, `schedule@${PHONE}`].sort(),
    );
    // A device rule no newer than the family-wide one is already replaced.
    expect(h.gk.recordRules(subject, [{ event: clause(wSk, "schedule", T - 20, { stale: 1 }), machine: PHONE }])).toEqual([]);
    h.published.length = 0;
    await h.gk.sendRoster();
    const opened = unwrapToWorkingKey(h.published[0].wrap, W2sk)!;
    const sent = JSON.parse(opened.inner.content).children[0];
    expect(sent.rules.map((e: SignedEvent) => e.id)).toEqual([old.id]);
    expect(sent.deviceRules.map((d: { machine: string; rule: SignedEvent }) => [d.machine, d.rule.id]).sort()).toEqual(
      [[LAPTOP, onLaptop.id], [PHONE, onPhone.id]].sort(),
    );

    // The other install takes them by device too.
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
    other.anchorRoot(R);
    expect(other.receiveRoster({ sealAuthor: W, inner: opened.inner })).toMatchObject({ ok: true });
    expect(Object.keys(keys2.rules[subject]).sort()).toEqual(["schedule", `schedule@${LAPTOP}`, `schedule@${PHONE}`].sort());

    // A newer family-wide schedule went to every device: the device rules go.
    const shared = clause(wSk, "schedule", T);
    expect(h.gk.recordRules(subject, [{ event: shared }])).toEqual(["schedule", `schedule@${LAPTOP}`, `schedule@${PHONE}`].sort());
    expect(h.gk.rulesFor(subject).map((r) => [r.event.id, r.machine])).toEqual([[shared.id, undefined]]);
  });

  it("sends one roster per child, and a joining install holds each until a ward of its own reports", async () => {
    const W2sk = generateSecretKey();
    const W2 = getPublicKey(W2sk);
    const guardians = { roots: [{ root: R, keys: [W] }, { root: R2, keys: [W2] }], pending: [], tombstones: 0, relays: [] };
    const sam = "aa".repeat(32);
    const ali = "bb".repeat(32);
    const h = harness([
      { id: "c1", name: "Sam", colour: "#000", rosterId: sam, devices: [{ ...readyWard(PHONE, "android"), guardians }] },
      { id: "c2", name: "Ali", colour: "#111", rosterId: ali, devices: [] },
    ]);
    await connectSignetRoot(h);
    h.gk.anchorRoot(R2);
    h.published.length = 0;
    const report = await h.gk.sendRoster();
    expect(report.events).toHaveLength(2);
    expect(report.sent).toEqual([W2]);
    const opened = h.published.map((p) => unwrapToWorkingKey(p.wrap, W2sk)!);
    expect(opened.map((o) => JSON.parse(o.inner.content).children.map((c: { subject: string }) => c.subject))).toEqual([[sam], [ali]]);

    // A joining install whose own ward hasn't reported holds both, not just the last.
    let keys2 = emptyGuardianKeys();
    const other = new GuardianKeys({
      read: () => ({ keys: keys2, children: [] }),
      write: (f) => {
        keys2 = f(keys2);
      },
      workingKey: () => W2sk,
      root: () => opsOf(r2Sk),
      publish: async () => {},
      now: () => T,
      defaultRelays: [],
    });
    other.anchorRoot(R);
    for (const o of opened) expect(other.receiveRoster({ sealAuthor: W, inner: o.inner })).toEqual({ ok: false, reason: "held" });
    expect(Object.keys(keys2.heldRosters).sort()).toEqual([`${W}:${ali}`, `${W}:${sam}`].sort());
  });

  it("keeps a child's roster under 48 KB: device rules go first, then the oldest family-wide rules", () => {
    const approval = clause(wSk, "approval", T);
    const base = { subject: "aa".repeat(32), issuedAt: T, name: "Sam", colour: "#000", devices: [] };
    const pad = "x".repeat(12_000);
    const fam = [0, 1, 2, 3].map((i) => ({ event: clause(wSk, `k${"abcd"[i]}`, T - 100 + i, { pad }) }));
    const dev = [{ event: clause(wSk, "schedule", T, { pad }), machine: PHONE }, { event: clause(wSk, "schedule", T, { pad }), machine: LAPTOP }];
    const all = [...fam, ...dev];
    const small = fitRoster(base, all, { approval, createdAt: T, pubkey: W }, 1_000_000);
    expect(small.dropped).toEqual([]);
    const fitted = fitRoster(base, all, { approval, createdAt: T, pubkey: W });
    // Six rules of ~12 KB don't fit in 48 KB; four still don't, three do.
    expect(fitted.dropped.slice(0, 2).sort()).toEqual([`schedule@${LAPTOP}`, `schedule@${PHONE}`].sort());
    expect(fitted.dropped.slice(2)).toEqual(["ka"]);
    expect(fitted.child.deviceRules).toBeUndefined();
    const size = new TextEncoder().encode(
      JSON.stringify({ ...familyRosterTemplate({ children: [fitted.child], approval, createdAt: T }), id: "0".repeat(64), pubkey: W, sig: "0".repeat(128) }),
    ).length;
    expect(size).toBeLessThanOrEqual(MAX_ROSTER_BYTES);
    expect(fitted.child.rules!.length).toBeGreaterThan(0);
  });

  it("settles a family-wide and a device rule of the same second by the lower event id, in either order", async () => {
    const subject = "aa".repeat(32);
    const sk = generateSecretKey();
    // Two clauses of the same second, one family-wide, one for the phone.
    let fam = clause(sk, "budget", T, { a: 1 });
    let dev = clause(sk, "budget", T, { b: 1 });
    if (fam.id > dev.id) [fam, dev] = [dev, fam];
    // Now `fam` has the lower id: it wins both ways round.
    const kept = (entries: { event: SignedEvent; machine?: string }[]) => {
      const h = harness([]);
      for (const e of entries) h.gk.recordRules(subject, [e]);
      return h.gk.rulesFor(subject).map((r) => [r.event.id, r.machine]);
    };
    expect(kept([{ event: fam }, { event: dev, machine: PHONE }])).toEqual([[fam.id, undefined]]);
    expect(kept([{ event: dev, machine: PHONE }, { event: fam }])).toEqual([[fam.id, undefined]]);
    // And with the lower id on the device rule, the device rule stays both ways round.
    expect(kept([{ event: dev }, { event: fam, machine: PHONE }]).sort()).toEqual([[dev.id, undefined], [fam.id, PHONE]].sort());
    expect(kept([{ event: fam, machine: PHONE }, { event: dev }]).sort()).toEqual([[dev.id, undefined], [fam.id, PHONE]].sort());
  });

  it("seeds each received rule once: a tie goes to the lower event id, a later local edit wins", async () => {
    const subject = "aa".repeat(32);
    const h = harness([]);
    const other = generateSecretKey();
    const mine = clause(wSk, "schedule", T, { mine: 1 });
    const same = clause(other, "schedule", T, { theirs: 1 });
    expect(h.gk.recordRules(subject, [{ event: mine }], { local: true })).toEqual(["schedule"]);
    // A received rule of the same second wins only with the lower id, as on the wards.
    expect(h.gk.recordRules(subject, [{ event: same }])).toEqual(same.id < mine.id ? ["schedule"] : []);
    const low = same.id < mine.id ? same : mine;
    expect(h.gk.ruleInForce(subject, "schedule")).toMatchObject({ local: low === mine, event: { id: low.id } });
    // The other order settles the same way.
    const h2 = harness([]);
    h2.gk.recordRules(subject, [{ event: same }]);
    h2.gk.recordRules(subject, [{ event: mine }], { local: true });
    expect(h2.gk.ruleInForce(subject, "schedule")!.event.id).toBe(low.id);

    // A newer received one wins, and is to be put on the screens once.
    const theirs = clause(other, "schedule", T + 5);
    expect(h.gk.recordRules(subject, [{ event: theirs }])).toEqual(["schedule"]);
    expect(Object.keys(h.gk.rulesToApply(subject))).toEqual(["schedule"]);
    h.gk.markRulesApplied(subject, { schedule: theirs.id });
    expect(h.gk.rulesToApply(subject)).toEqual({});
    expect(h.gk.ruleInForce(subject, "schedule", PHONE)).toMatchObject({ key: "schedule", local: false });

    // A local edit made after it wins.
    const later = clause(wSk, "schedule", T + 6);
    expect(h.gk.recordRules(subject, [{ event: later }], { local: true })).toEqual(["schedule"]);
    expect(h.gk.ruleInForce(subject, "schedule")).toMatchObject({ local: true });
  });

  it("holds no rules for a child it was never told about", async () => {
    const h = harness([]);
    expect(h.gk.rulesFor("aa".repeat(32))).toEqual([]);
  });
});
