// Phones that paired with this guardian but are missing from their records.
//
// Pairing can split-brain: the phone completes its half (it holds the guardian
// key, polls, and heartbeats every 60s) while Kintrinsic never records a device
// for it. The guardian's app then has no recipient to seal clauses to, so every
// rule silently goes nowhere — Mia's phone sat like this on 2026-07-27
// with `clausesSeen=0` while her 15-minute limit "saved" fine.
//
// Nothing self-healed it: `SET_DEVICE_LAST_SEEN` only maps over devices that
// already exist, so heartbeats from an unknown machine were dropped on the
// floor. But `setDeviceStatus` kept every one of them, keyed by machine — the
// evidence was already in the guardian's hand, just unused. This turns it into
// the recovery.
//
// SAFETY — and the mistake this file used to make (S2, review 2026-08-07).
//
// It used to reason: "these statuses arrived as 1059 wraps sealed to the
// guardian's pubkey, so a device that produced one was handed the pairing link
// BY this guardian." That does not follow, and it is the more dangerous kind
// of wrong: it reads like a proof.
//
// Sealing to the guardian needs only the guardian's PUBLIC key, which is not a
// secret — every 1059 wrap on a public relay carries it in a cleartext `p`
// tag. `unwrapStatus` verifies that the seal is signed and that
// `rumor.pubkey == seal.pubkey == payload.machine`; all three can be one
// attacker's own fresh keypair. Anyone at all could mint a keypair, heartbeat
// forged STATUS at a guardian, and wait for the banner to appear — which it
// would, exactly when the guardian was expecting a phone to show up. One tap
// and the child's entire standing charter goes to a stranger's machine while
// the real phone gets nothing.
//
// So authenticity of the WRAP is not proof of PAIRING, and only pairing is
// worth anything here. What is proof: the one-time token. It is 128 bits of
// CSPRNG that this guardian minted and put on screen (or down a cable), the
// device echoes it on STATUS for a bounded window after pairing, and it is
// carried inside the encryption — unreachable to anyone who was not handed it.
// A machine is offered for adoption if and only if it echoes a token this
// guardian actually minted (see ./pairTokens).
//
// A guardian whose token has aged out is not stuck: showing the pairing code
// again mints a fresh one and the phone echoes it on its next heartbeat. The
// recovery costs one more tap; the hole it closes cost a child's whole charter.

import type { Child } from "../domain/types";
import type { DeviceStatus } from "../wire/status";

/** A phone heard from, that no child in the family lays claim to. */
export interface UnclaimedDevice {
  /** The device's own pubkey (== `DeviceStatus.machine`) — the wrap recipient. */
  machine: string;
  /** Epoch ms, converted from the device's unix-seconds clock. */
  lastSeenAt: number;
  /** What it reports running, shown verbatim so the guardian can recognise it. */
  appVersionName?: string;
  /** The minted token it echoed — the reason it is offered at all. Carried so
   *  the claim can re-check it at the moment of binding, and spend it after. */
  pairToken: string;
}

/** How recently a machine must have beaten to be offered. A split-brain phone
 *  heartbeats every 60s, so three missed beats means it is not in the state
 *  this recovery exists for — while a phone the guardian deliberately RELEASED
 *  goes quiet at that instant, and re-offering it would record "paired"
 *  against a device listening to nothing: split-brain in the inverse
 *  direction. Silence is the tell between the two. */
export const UNCLAIMED_FRESH_MS = 3 * 60_000;

/**
 * Every machine heard from RECENTLY, echoing a pairing token THIS guardian
 * minted, that no child's device list references.
 *
 * All three conditions carry weight. Unreferenced is the symptom; recent is
 * what separates a phone mid-setup from one the guardian deliberately released
 * (which goes quiet at that instant, and re-offering it would record "paired"
 * against a device listening to nothing — split-brain in the inverse
 * direction); and the token is the only one of the three that is PROOF rather
 * than circumstance. See the header for why the wrap is not.
 *
 * Deliberately NOT matched to a child by `DeviceStatus.subject`: a child is a
 * local label at this stage and `Child.dependantPubkey` is always null, so there
 * is nothing to match against. The guardian says whose phone it is — the app
 * guessing would be the same class of mistake as assuming the pairing worked.
 *
 * Newest first: the phone someone is holding is the one that just beat.
 */
export function unclaimedDevices(
  children: Child[],
  deviceStatus: Record<string, DeviceStatus>,
  /** Tokens this guardian minted and has not aged out (`livePairTokens`). An
   *  empty set offers nothing — the fail-closed direction, and the one a
   *  guardian can always get out of by showing the pairing code again. */
  pairTokens: ReadonlySet<string>,
  now: number = Date.now(),
): UnclaimedDevice[] {
  const claimed = new Set(
    children.flatMap((c) =>
      c.devices
        .map((d) => d.devicePubkey)
        .filter((k): k is string => typeof k === "string" && k.length > 0),
    ),
  );
  return Object.values(deviceStatus)
    .filter((s) => !claimed.has(s.machine))
    .filter((s) => now - s.ts * 1000 <= UNCLAIMED_FRESH_MS)
    .filter((s) => !!s.pairToken && pairTokens.has(s.pairToken))
    .map((s) => ({
      machine: s.machine,
      lastSeenAt: s.ts * 1000,
      appVersionName: s.appVersionName,
      pairToken: s.pairToken!,
    }))
    .sort((a, b) => b.lastSeenAt - a.lastSeenAt);
}

/** Short form for the prompt — a guardian matches a phone by the code it shows. */
export function shortMachine(machine: string): string {
  return machine.length <= 16 ? machine : `${machine.slice(0, 8)}…${machine.slice(-8)}`;
}
