import { describe, expect, it } from "vitest";
import { emptyGuardianKeys } from "../keys/state";
import { parsePresenceCode } from "../keys/presenceCode";
import {
  androidWardsSettingPin,
  androidWardsPinReplaced,
  androidWardsPinUnconfirmed,
  androidWardsWithoutPin,
  clockBehindLine,
  clockBehindWards,
  CLOCK_NOTICE_SECONDS,
  pinStateOf,
  pinReplacedLine,
  pinUnconfirmedLine,
  durationWords,
  familyWards,
  guardiansSubroute,
  leaveGate,
  moveGate,
  pendingFor,
  pinProblem,
  presenceCodeFrom,
  removalsAgainstMe,
  removeGate,
  rootsView,
  stripNostrPrefix,
  typedPresenceItems,
} from "./guardiansView";
import {
  LAPTOP,
  ME,
  MY_OTHER_W,
  MY_W,
  OTHER,
  OTHER_W,
  PHONE,
  STATEMENT,
  TOKEN,
  device,
  family,
  guardians,
  signetRoot,
} from "./guardiansTestKit";

const removalOfMe = `kintrinsic/unguardian/${ME}`;
const removalOfOther = `kintrinsic/unguardian/${OTHER}`;

describe("guardians view", () => {
  it("lists the family's guardians, yours first, with each one's phones", () => {
    const wards = familyWards(family([device("Laptop", LAPTOP, "linux", guardians())]));
    const roots = rootsView(wards, ME);
    expect(roots.map((r) => [r.root, r.you])).toEqual([
      [ME, true],
      [OTHER, false],
    ]);
    expect(roots[0].keys).toEqual([MY_W, MY_OTHER_W].sort());
    expect(roots[1].keys).toEqual([OTHER_W]);
  });

  it("leaves out unpaired and departed devices", () => {
    const departed = { ...device("Old", PHONE, "android", guardians()), departedAt: 5 };
    const wards = familyWards(family([departed, { ...device("New", LAPTOP, "linux"), pairing: "unpaired" as const }]));
    expect(wards).toEqual([]);
  });

  describe("leaving", () => {
    it("is blocked when no device has reported its guardians", () => {
      const gate = leaveGate(familyWards(family([device("Laptop", LAPTOP, "linux")])), ME);
      expect(gate).toMatchObject({ ok: false });
    });

    it("is blocked on a device where you're the last guardian, naming it", () => {
      const alone = guardians({ roots: [{ root: ME, keys: [MY_W] }] });
      const wards = familyWards(
        family([device("Laptop", LAPTOP, "linux", guardians()), device("Phone", PHONE, "android", alone)]),
      );
      const gate = leaveGate(wards, ME);
      expect(gate.ok).toBe(false);
      expect(!gate.ok && gate.reason).toBe("You're the only guardian on Phone. Unpair it instead.");
    });

    it("is open when every reporting device keeps another guardian", () => {
      expect(leaveGate(familyWards(family([device("Laptop", LAPTOP, "linux", guardians())])), ME)).toEqual({ ok: true });
    });
  });

  it("offers removing the other parent to a key kept only on this phone, and gates moving", () => {
    expect(removeGate(emptyGuardianKeys()).ok).toBe(true);
    expect(removeGate({ ...emptyGuardianKeys(), root: signetRoot() }).ok).toBe(true);
    expect(moveGate(emptyGuardianKeys(), true).ok).toBe(true);
    expect(moveGate({ ...emptyGuardianKeys(), root: signetRoot() }, false).ok).toBe(false);
    expect(moveGate({ ...emptyGuardianKeys(), root: signetRoot() }, true).ok).toBe(true);
  });

  it("marks a pending removal of you, and your own pending items with their statement", () => {
    const keys = {
      ...emptyGuardianKeys(),
      root: signetRoot(),
      sent: { [removalOfOther]: { id: STATEMENT, type: "removal" as const, issuedAt: 1, key: OTHER } },
    };
    const g = guardians({
      pending: [
        { d: removalOfMe, kind: "removal", takesEffectAt: 500, expiresAt: 900 },
        { d: removalOfOther, kind: "removal", takesEffectAt: 400, expiresAt: 800 },
      ],
    });
    const items = pendingFor(g, keys, ME, 100);
    expect(items.map((p) => [p.d, p.role, p.statement])).toEqual([
      [removalOfMe, "against-me", undefined],
      [removalOfOther, "mine", STATEMENT],
    ]);
    // Past its expiresAt the device has dropped it, so it isn't shown.
    expect(pendingFor(g, keys, ME, 850).map((p) => p.d)).toEqual([removalOfMe]);
    expect(pendingFor(g, keys, ME, 900)).toEqual([]);
    const wards = familyWards(family([device("Laptop", LAPTOP, "linux", g), device("Phone", PHONE, "android", g)]));
    expect(removalsAgainstMe(wards, keys, ME, 100, MY_W)).toEqual([
      {
        d: removalOfMe,
        devices: ["Laptop", "Phone"],
        children: ["Robin"],
        takesEffectAt: 500,
        remover: OTHER,
        objected: false,
        candidates: [OTHER],
      },
    ]);
    // Objected at one device only: not yet "objected".
    const one = { ...keys, objected: { [LAPTOP]: [removalOfMe] } };
    expect(removalsAgainstMe(wards, one, ME, 100, MY_W)[0].objected).toBe(false);
    const both = { ...keys, objected: { [LAPTOP]: [removalOfMe], [PHONE]: [removalOfMe] } };
    expect(removalsAgainstMe(wards, both, ME, 100, MY_W)[0].objected).toBe(true);
    // A device that doesn't list this phone's key: it can't object on its own.
    expect(removalsAgainstMe(wards, keys, ME, 100, "ab".repeat(32))[0].blocked).toBe("key");
    expect(removalsAgainstMe(wards, both, ME, 900)).toEqual([]);
  });

  it("offers the typed code every reported item this phone can confirm, by the device's id (D10)", () => {
    const ID = "9".repeat(64);
    const inviteOfMe = `kintrinsic/guardian/${ME}`;
    const inviteOfOther = `kintrinsic/guardian/${OTHER}`;
    const removalOfOther = `kintrinsic/unguardian/${OTHER}`;
    const removalOfMe = `kintrinsic/unguardian/${ME}`;
    const g = guardians({
      pending: [
        { d: inviteOfMe, id: ID, kind: "coguardian", expiresAt: 900 },
        { d: inviteOfOther, id: ID, kind: "coguardian", expiresAt: 900 },
        { d: removalOfOther, id: ID, kind: "removal", expiresAt: 900 },
        { d: removalOfMe, id: ID, kind: "removal", expiresAt: 900 },
      ],
    });
    const wards = familyWards(family([device("Laptop", LAPTOP, "linux", g)]));
    const keys = { ...emptyGuardianKeys(), root: signetRoot() };
    // An invitation of this phone's identity, though this phone sent nothing; not
    // the other parent's invitation, nor a removal someone else started.
    expect(typedPresenceItems(wards, keys, ME, 100).map((i) => [i.d, i.statement, i.machine])).toEqual([[inviteOfMe, ID, LAPTOP]]);
    // A removal this phone started: offered, with the device's id.
    const sent = { ...keys, sent: { [removalOfOther]: { id: ID, type: "removal" as const, issuedAt: 1, key: OTHER } } };
    expect(typedPresenceItems(wards, sent, ME, 100).map((i) => i.d)).toEqual([inviteOfMe, removalOfOther]);
    // The device's id names a different statement than this phone sent: not this phone's.
    const stale = { ...keys, sent: { [removalOfOther]: { id: STATEMENT, type: "removal" as const, issuedAt: 1, key: OTHER } } };
    expect(typedPresenceItems(wards, stale, ME, 100).map((i) => i.d)).toEqual([inviteOfMe]);
    // A device that reports no ids: only what this phone sent, by its own record.
    const old = familyWards(family([device("Laptop", LAPTOP, "linux", guardians({ pending: [{ d: inviteOfMe, kind: "coguardian", expiresAt: 900 }] }))]));
    expect(typedPresenceItems(old, keys, ME, 100)).toEqual([]);
    expect(typedPresenceItems(wards, keys, ME, 900)).toEqual([]);
  });

  it("finds Android wards with no guardian PIN from their own STATUS (D4)", () => {
    const g = (pinSet?: boolean) => ({ roots: [], pending: [], tombstones: 0, relays: [], ...(pinSet !== undefined ? { pinSet } : {}) });
    const wardsWith = (pinSet?: boolean) =>
      familyWards(family([device("Laptop", LAPTOP, "linux"), device("Phone", PHONE, "android", g(pinSet))]));
    const none = emptyGuardianKeys();
    // Never said, or says none: ask. Says it holds one (whoever set it): don't.
    expect(androidWardsWithoutPin(familyWards(family([device("Phone", PHONE, "android")])), none).map((w) => w.device.label)).toEqual(["Phone"]);
    expect(androidWardsWithoutPin(wardsWith(undefined), none).map((w) => w.device.label)).toEqual(["Phone"]);
    expect(androidWardsWithoutPin(wardsWith(false), none).map((w) => w.device.label)).toEqual(["Phone"]);
    expect(androidWardsWithoutPin(wardsWith(true), none)).toEqual([]);
    // A PIN on its way isn't asked for again; once "set" here, the ward's word decides.
    const sending = { ...none, pins: { [PHONE]: { state: "sending" as const, sentAt: 1 } } };
    expect(androidWardsWithoutPin(wardsWith(false), sending)).toEqual([]);
    expect(androidWardsSettingPin(wardsWith(false), sending).map((w) => w.device.label)).toEqual(["Phone"]);
    // The ward's pinSet alone may be an older PIN: still sending until it names this one (SF2).
    expect(pinStateOf(wardsWith(true)[1], sending)).toBe("sending");
    const set = { ...none, pins: { [PHONE]: { state: "set" as const, sentAt: 1 } } };
    expect(androidWardsWithoutPin(wardsWith(false), set).map((w) => w.device.label)).toEqual(["Phone"]);
    // Resent as often as it will be and never confirmed: asked again, and said.
    const gaveUp = { ...none, pins: { [PHONE]: { state: "unconfirmed" as const, sentAt: 1 } } };
    expect(pinStateOf(wardsWith(false)[1], gaveUp)).toBe("unconfirmed");
    expect(androidWardsWithoutPin(wardsWith(false), gaveUp).map((w) => w.device.label)).toEqual(["Phone"]);
    expect(androidWardsPinUnconfirmed(wardsWith(false), gaveUp).map((w) => w.device.label)).toEqual(["Phone"]);
    expect(androidWardsPinUnconfirmed(wardsWith(false), sending)).toEqual([]);
    expect(pinUnconfirmedLine("Sam's phone")).toBe("Couldn't confirm the guardian PIN on Sam's phone. Open the PIN screen to try again.");
    // Another install's newer PIN took the ward: it holds one, and the guardian is told.
    const replaced = { ...none, pins: { [PHONE]: { state: "replaced" as const, sentAt: 1 } } };
    expect(pinStateOf(wardsWith(true)[1], replaced)).toBe("set");
    expect(androidWardsWithoutPin(wardsWith(true), replaced)).toEqual([]);
    expect(androidWardsPinReplaced(wardsWith(true), replaced).map((w) => w.device.label)).toEqual(["Phone"]);
    expect(androidWardsPinReplaced(wardsWith(true), sending)).toEqual([]);
    expect(pinReplacedLine("Sam's phone")).toBe("The other parent changed the PIN on Sam's phone.");
  });

  it("checks a guardian PIN draft", () => {
    expect(pinProblem("12345", "12345")).toBe("Choose a PIN of at least 6 digits.");
    expect(pinProblem("12345a", "12345a")).toBe("Use digits only.");
    expect(pinProblem("123456", "123457")).toBe("The two PINs don't match.");
    expect(pinProblem("123456", "123456")).toBeNull();
  });

  it("builds a presence code the parser reads back", () => {
    const code = presenceCodeFrom(LAPTOP, TOKEN, removalOfOther, STATEMENT);
    expect(parsePresenceCode(code)).toMatchObject({ machine: LAPTOP, token: TOKEN, d: removalOfOther, statement: STATEMENT });
  });

  it("words durations and strips a nostr: prefix", () => {
    expect(durationWords(72 * 3600)).toBe("3 days");
    expect(durationWords(26 * 3600 + 60)).toBe("1 day 2 hours");
    expect(durationWords(90)).toBe("1 minute");
    expect(durationWords(10)).toBe("less than a minute");
    expect(stripNostrPrefix(" nostr:npub1xyz ")).toBe("npub1xyz");
  });

  it("finds the Guardians screens under Settings", () => {
    expect(guardiansSubroute("#/settings/guardians")).toBe("");
    expect(guardiansSubroute("#/settings/guardians/join")).toBe("join");
    expect(guardiansSubroute("#/settings")).toBeNull();
    expect(guardiansSubroute("#/settings/add-child")).toBeNull();
  });

  it("names the remover the device reports, and asks who when it can't tell (follow-ups to Q1)", () => {
    const THIRD = "6".repeat(64);
    const keys = { ...emptyGuardianKeys(), root: signetRoot() };
    const three = (signer?: string) =>
      guardians({
        roots: [
          { root: ME, keys: [MY_W] },
          { root: OTHER, keys: [] },
          { root: THIRD, keys: [] },
        ],
        pending: [{ d: removalOfMe, kind: "removal", expiresAt: 900, ...(signer ? { signer } : {}) }],
      });
    const named = familyWards(family([device("Laptop", LAPTOP, "linux", three(THIRD))]));
    expect(removalsAgainstMe(named, keys, ME, 100, MY_W)[0]).toMatchObject({ remover: THIRD });
    expect(removalsAgainstMe(named, keys, ME, 100, MY_W)[0].blocked).toBeUndefined();
    const unknown = familyWards(family([device("Laptop", LAPTOP, "linux", three())]));
    const r = removalsAgainstMe(unknown, keys, ME, 100, MY_W)[0];
    expect(r.remover).toBeUndefined();
    expect(r.blocked).toBe("remover");
    expect(r.candidates.sort()).toEqual([OTHER, THIRD].sort());
  });

  it("shows a device's clock notice only after its clamp counter rose, for a few days", () => {
    const wards = familyWards(family([device("Robin's phone", PHONE, "android", guardians())]));
    const keys = { ...emptyGuardianKeys(), clamped: { [PHONE]: { count: 2, roseAt: 1000 } } };
    expect(clockBehindWards(wards, keys, 1000 + 60).map((w) => w.device.label)).toEqual(["Robin's phone"]);
    expect(clockBehindWards(wards, keys, 1000 + CLOCK_NOTICE_SECONDS)).toEqual([]);
    expect(clockBehindWards(wards, { ...emptyGuardianKeys(), clamped: { [PHONE]: { count: 2 } } }, 1060)).toEqual([]);
    expect(clockBehindLine("Robin's phone")).toBe(
      "Robin's phone's clock is behind this phone's; rules still apply, in the order they arrive.",
    );
  });
});
