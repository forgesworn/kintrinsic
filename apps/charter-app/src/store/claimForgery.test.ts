// S2 (review 2026-08-07) — the forged-heartbeat attack on the unclaimed-device
// claim flow, exercised end-to-end rather than from a hand-written object.
//
// The point of doing it this way: the attacker's STATUS is not malformed and
// is not rejected anywhere. It is a real NIP-59 gift-wrap, sealed to the
// guardian's real public key, signed by a real key, with the author binding
// `rumor.pubkey == seal.pubkey == payload.machine` satisfied exactly as the
// contract requires. `unwrapStatus` opens it and returns it, correctly. Every
// crypto check in the pipeline passes, because none of them was ever asking
// the question that matters here — was this device handed a pairing link BY
// this guardian? Only the one-time token answers that.
//
// A guardian's pubkey is not a secret: every 1059 wrap on a public relay
// carries it in a cleartext `p` tag, so "could seal to the guardian" is a
// property any stranger on the internet has.

import { describe, expect, it } from "vitest";
import { finalizeEvent, generateSecretKey, getPublicKey, nip44 } from "nostr-tools";
import { CHARTER_DEVICE_STATUS, unwrapStatus, type DeviceStatus } from "../wire/status";
import { unclaimedDevices } from "./unclaimedDevices";
import type { Child } from "../domain/types";

const SEAL = 13;
const GIFT_WRAP = 1059;
const MARKER: [string, string] = ["t", "charter-device"];
const NOW = 1_700_000_000;
/** A token this guardian actually minted and put on a QR. */
const MINTED_TOKEN = "0123456789abcdef0123456789abcdef";

function statusFor(machine: string, pairToken?: string): DeviceStatus {
  return {
    v: 1,
    subject: "c".repeat(64),
    machine,
    ts: NOW,
    dayKey: "2026-08-07",
    usedTodaySecs: 600,
    windowLeftSecs: 1800,
    quotaLeftSecs: 900,
    effectiveSecs: 900,
    locked: false,
    source: "guardian",
    appVersionName: "0.7.5",
    ...(pairToken ? { pairToken } : {}),
  };
}

/** Exactly what a real device emits — and exactly what an attacker can emit. */
function giftWrapStatus(status: DeviceStatus, machineSk: Uint8Array, guardianPk: string) {
  const rumor = {
    kind: CHARTER_DEVICE_STATUS,
    pubkey: getPublicKey(machineSk),
    created_at: NOW,
    tags: [MARKER],
    content: JSON.stringify(status),
  };
  const seal = finalizeEvent(
    {
      kind: SEAL,
      created_at: NOW,
      tags: [],
      content: nip44.encrypt(JSON.stringify(rumor), nip44.getConversationKey(machineSk, guardianPk)),
    },
    machineSk,
  );
  const eph = generateSecretKey();
  return finalizeEvent(
    {
      kind: GIFT_WRAP,
      created_at: NOW,
      tags: [["p", guardianPk]],
      content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(eph, guardianPk)),
    },
    eph,
  );
}

const child = (): Child => ({
  id: "child_mia",
  name: "Mia",
  color: "#fff",
  dependantPubkey: null,
  devices: [],
  policies: [],
});

describe("a stranger forging STATUS at a guardian", () => {
  const guardianSk = generateSecretKey();
  const guardianPk = getPublicKey(guardianSk);

  it("produces a wrap the guardian's app opens and accepts as authentic", () => {
    const attackerSk = generateSecretKey();
    const wrap = giftWrapStatus(
      statusFor(getPublicKey(attackerSk)),
      attackerSk,
      guardianPk,
    );
    // Not a bug, and not the thing to fix: this SHOULD unwrap. The wrap is
    // genuinely well-formed and genuinely authored by the key it claims.
    expect(unwrapStatus(wrap, guardianSk, NOW)).not.toBeNull();
  });

  it("is NOT offered for adoption, because it can echo no minted token", () => {
    const attackerSk = generateSecretKey();
    const machine = getPublicKey(attackerSk);
    const opened = unwrapStatus(
      giftWrapStatus(statusFor(machine), attackerSk, guardianPk),
      guardianSk,
      NOW,
    );
    expect(opened).not.toBeNull();
    expect(
      unclaimedDevices([child()], { [machine]: opened! }, new Set([MINTED_TOKEN]), NOW * 1000),
    ).toEqual([]);
  });

  it("cannot guess its way in by inventing a token", () => {
    const attackerSk = generateSecretKey();
    const machine = getPublicKey(attackerSk);
    for (const guess of ["", "token", MINTED_TOKEN.slice(0, -1), MINTED_TOKEN.toUpperCase()]) {
      const opened = unwrapStatus(
        giftWrapStatus(statusFor(machine, guess), attackerSk, guardianPk),
        guardianSk,
        NOW,
      );
      expect(
        unclaimedDevices([child()], { [machine]: opened! }, new Set([MINTED_TOKEN]), NOW * 1000),
      ).toEqual([]);
    }
  });

  /*
   * The other half of the proof: the gate must not have simply broken the
   * feature it protects. A phone that really did scan the guardian's QR
   * carries the token, and is still offered — which is the recovery working.
   */
  it("does not block the real phone the recovery exists for", () => {
    const phoneSk = generateSecretKey();
    const machine = getPublicKey(phoneSk);
    const opened = unwrapStatus(
      giftWrapStatus(statusFor(machine, MINTED_TOKEN), phoneSk, guardianPk),
      guardianSk,
      NOW,
    );
    const offered = unclaimedDevices(
      [child()],
      { [machine]: opened! },
      new Set([MINTED_TOKEN]),
      NOW * 1000,
    );
    expect(offered.map((d) => d.machine)).toEqual([machine]);
  });

  /*
   * And the attack in its most convincing dress: the forgery arrives WITH the
   * real phone, beating at the same moment, at exactly the point in a set-up
   * where a parent is watching the screen waiting for a phone to appear.
   */
  it("loses to the real phone even when both are heartbeating at once", () => {
    const phoneSk = generateSecretKey();
    const attackerSk = generateSecretKey();
    const phone = getPublicKey(phoneSk);
    const attacker = getPublicKey(attackerSk);
    const open = (sk: Uint8Array, token?: string) =>
      unwrapStatus(giftWrapStatus(statusFor(getPublicKey(sk), token), sk, guardianPk), guardianSk, NOW)!;
    const offered = unclaimedDevices(
      [child()],
      { [phone]: open(phoneSk, MINTED_TOKEN), [attacker]: open(attackerSk) },
      new Set([MINTED_TOKEN]),
      NOW * 1000,
    );
    expect(offered.map((d) => d.machine)).toEqual([phone]);
  });
});
