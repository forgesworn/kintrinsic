import { describe, expect, it } from "vitest";
import { shortMachine, unclaimedDevices, UNCLAIMED_FRESH_MS } from "./unclaimedDevices";
import type { Child } from "../domain/types";
import type { DeviceStatus } from "../wire/status";

const M1 = "a".repeat(64);
const M2 = "b".repeat(64);
/** A token this guardian minted — the proof of pairing (S2). */
const TOKEN = "0123456789abcdef0123456789abcdef";
const TOKEN2 = "fedcba9876543210fedcba9876543210";
const MINTED = new Set([TOKEN, TOKEN2]);

/** `pairToken: null` means the heartbeat echoed nothing at all — distinct
 *  from omitting the argument, which gives the ordinary minted token. */
function status(
  machine: string,
  ts: number,
  appVersionName?: string,
  pairToken: string | null = TOKEN,
): DeviceStatus {
  return {
    v: 1,
    subject: "c".repeat(64),
    machine,
    ts,
    dayKey: "2026-07-27",
    usedTodaySecs: 0,
    appVersionName,
    pairToken: pairToken ?? undefined,
  } as DeviceStatus;
}

function child(over: Partial<Child> = {}): Child {
  return {
    id: "child_mia",
    name: "Mia",
    color: "#fff",
    dependantPubkey: null,
    devices: [],
    policies: [],
    ...over,
  };
}

describe("unclaimedDevices", () => {
  // The exact split-brain: her phone heartbeating happily, no device record.
  it("surfaces a phone that heartbeats but is in nobody's device list", () => {
    const got = unclaimedDevices(
      [child()],
      { [M1]: status(M1, 1_700_000_000) },
      MINTED,
      1_700_000_000 * 1000,
    );
    expect(got).toHaveLength(1);
    expect(got[0].machine).toBe(M1);
    expect(got[0].lastSeenAt).toBe(1_700_000_000 * 1000);
  });

  it("stays quiet once the phone is claimed", () => {
    const claimed = child({
      devices: [
        {
          id: "dev1",
          label: "Mia’s phone",
          platform: "android",
          pairing: "paired",
          devicePubkey: M1,
        },
      ],
    });
    expect(
      unclaimedDevices([claimed], { [M1]: status(M1, 1_700_000_000) }, MINTED, 1_700_000_000 * 1000),
    ).toEqual([]);
  });

  // A phone belonging to a sibling must not be offered again under this child.
  it("treats a device claimed by ANY child as claimed", () => {
    const mia = child();
    const sibling = child({
      id: "child_two",
      name: "Rook",
      devices: [
        {
          id: "dev9",
          label: "Rook’s phone",
          platform: "android",
          pairing: "paired",
          devicePubkey: M1,
        },
      ],
    });
    expect(unclaimedDevices([mia, sibling], { [M1]: status(M1, 1) }, MINTED, 1000)).toEqual([]);
  });

  it("ignores a half-set-up device row that carries no key", () => {
    const halfSetUp = child({
      devices: [
        {
          id: "dev1",
          label: "new phone",
          platform: "android",
          pairing: "unpaired",
          devicePubkey: null,
        },
      ],
    });
    // The row has no key, so it claims nothing — the live phone still shows.
    expect(unclaimedDevices([halfSetUp], { [M1]: status(M1, 1) }, MINTED, 1000)).toHaveLength(1);
  });

  it("puts the most recent heartbeat first", () => {
    const got = unclaimedDevices(
      [child()],
      {
        [M1]: status(M1, 1_900),
        [M2]: status(M2, 2_000, undefined, TOKEN2),
      },
      MINTED,
      2_000 * 1000,
    );
    expect(got.map((d) => d.machine)).toEqual([M2, M1]);
  });

  it("carries the reported version so a guardian can recognise the phone", () => {
    const got = unclaimedDevices([child()], { [M1]: status(M1, 1, "0.4.1") }, MINTED, 1000);
    expect(got[0].appVersionName).toBe("0.4.1");
  });

  it("has nothing to say when no device has been heard from", () => {
    expect(unclaimedDevices([child()], {}, MINTED)).toEqual([]);
  });

  // A guardian who deliberately released a phone must not see it re-offered
  // seconds later: the phone honored the RELEASE and went quiet, so adopting
  // it back records "paired" against a device listening to nothing —
  // split-brain in the inverse direction. Silence is the tell: a real
  // split-brain phone beats every 60s; a released one stops.
  it("does not offer a machine that has gone quiet", () => {
    const now = 1_700_000_000 * 1000 + UNCLAIMED_FRESH_MS + 1;
    expect(unclaimedDevices([child()], { [M1]: status(M1, 1_700_000_000) }, MINTED, now)).toEqual(
      [],
    );
  });

  it("keeps offering a machine still beating within the window", () => {
    const now = 1_700_000_000 * 1000 + UNCLAIMED_FRESH_MS - 1;
    expect(
      unclaimedDevices([child()], { [M1]: status(M1, 1_700_000_000) }, MINTED, now),
    ).toHaveLength(1);
  });

  /*
   * S2 (review 2026-08-07) — THE attack this gate exists for.
   *
   * Every status here is a perfectly valid, correctly-sealed, correctly-signed
   * 1059 wrap that `unwrapStatus` accepts without complaint: the attacker
   * minted their own keypair, sealed to the guardian's PUBLIC key (readable off
   * any relay's cleartext `p` tags) and self-declared themselves as `machine`.
   * Authenticity of the wrap was never the question. Pairing is, and the token
   * is the only thing here that can answer it.
   */
  describe("a forged heartbeat from a stranger's keypair", () => {
    it("is not offered when it echoes no token at all", () => {
      const forged = status(M1, 1_700_000_000, "0.7.5", null);
      expect(
        unclaimedDevices([child()], { [M1]: forged }, MINTED, 1_700_000_000 * 1000),
      ).toEqual([]);
    });

    it("is not offered when it echoes a token this guardian never minted", () => {
      const forged = status(M1, 1_700_000_000, "0.7.5", "deadbeefdeadbeefdeadbeefdeadbeef");
      expect(
        unclaimedDevices([child()], { [M1]: forged }, MINTED, 1_700_000_000 * 1000),
      ).toEqual([]);
    });

    it("cannot ride in alongside the real phone it is impersonating", () => {
      const real = status(M1, 1_700_000_000, "0.7.5", TOKEN);
      const forged = status(M2, 1_700_000_100, "0.7.5", "not-a-minted-token");
      const got = unclaimedDevices(
        [child()],
        { [M1]: real, [M2]: forged },
        MINTED,
        1_700_000_100 * 1000,
      );
      // The forgery beat more recently, so a naive sort would put it FIRST —
      // under the guardian's thumb, at the exact moment they are expecting a
      // phone to appear.
      expect(got.map((d) => d.machine)).toEqual([M1]);
    });
  });

  /*
   * The fail-closed direction. An empty token ledger (a fresh browser, cleared
   * storage, a corrupt read) offers nothing at all rather than everything —
   * and the way out is a tap, not a support call: showing the pairing code
   * again mints a token the phone echoes on its next heartbeat.
   */
  it("offers nothing when this guardian has no live tokens", () => {
    expect(
      unclaimedDevices([child()], { [M1]: status(M1, 1_700_000_000) }, new Set(), 1_700_000_000 * 1000),
    ).toEqual([]);
  });

  it("reports the token that vouched for the machine, so the claim can re-check it", () => {
    const got = unclaimedDevices([child()], { [M1]: status(M1, 1) }, MINTED, 1000);
    expect(got[0].pairToken).toBe(TOKEN);
  });
});

describe("shortMachine", () => {
  it("abbreviates a 64-hex key head-and-tail", () => {
    expect(shortMachine(M1)).toBe(`${"a".repeat(8)}…${"a".repeat(8)}`);
  });

  it("leaves an already-short value alone", () => {
    expect(shortMachine("abc")).toBe("abc");
  });
});
