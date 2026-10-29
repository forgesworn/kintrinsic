import { describe, expect, it } from "vitest";
import { emptyGuardianKeys } from "../keys/state";
import { parsePresenceCode } from "../keys/presenceCode";
import {
  androidWardsWithoutPin,
  durationWords,
  familyWards,
  leaveGate,
  moveGate,
  pendingFor,
  pinProblem,
  presenceCodeFrom,
  removalsAgainstMe,
  removeGate,
  rootsView,
  stripNostrPrefix,
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

  it("refuses removing the other parent and moving for a key kept only on this phone", () => {
    expect(removeGate(emptyGuardianKeys()).ok).toBe(false);
    expect(removeGate({ ...emptyGuardianKeys(), root: signetRoot() }).ok).toBe(true);
    expect(moveGate(emptyGuardianKeys(), true).ok).toBe(false);
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
    const items = pendingFor(g, keys, ME);
    expect(items.map((p) => [p.d, p.role, p.statement])).toEqual([
      [removalOfMe, "against-me", undefined],
      [removalOfOther, "mine", STATEMENT],
    ]);
    const wards = familyWards(family([device("Laptop", LAPTOP, "linux", g), device("Phone", PHONE, "android", g)]));
    expect(removalsAgainstMe(wards, keys, ME)).toEqual([{ d: removalOfMe, devices: ["Laptop", "Phone"], takesEffectAt: 500 }]);
  });

  it("finds Android wards with no guardian PIN sent", () => {
    const wards = familyWards(family([device("Laptop", LAPTOP, "linux"), device("Phone", PHONE, "android")]));
    expect(androidWardsWithoutPin(wards, emptyGuardianKeys()).map((w) => w.device.label)).toEqual(["Phone"]);
    expect(androidWardsWithoutPin(wards, { ...emptyGuardianKeys(), pinSentAt: { [PHONE]: 1 } })).toEqual([]);
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
});
