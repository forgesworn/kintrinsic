// The Kintrinsic carrier APK injects `window.CharterCarrier` into this page
// (native Android shell, origin-locked to this console). When present, hand
// it the guardian key + relays so its foreground service can classify relay
// wraps and raise approval notifications while the app is closed.
//
// Read-only with respect to key state: NEVER mints (absent) and NEVER touches
// a corrupt value (the restore flow owns that) — mirrors readGuardianKey's
// no-silent-overwrite guarantee.

import { getPublicKey } from "nostr-tools";
import { readGuardianKey } from "../signer/guardianKey";
import { DEFAULT_RELAYS } from "../signer/config";
import type { Child } from "../domain/types";
import { buildCarrierRoster } from "./roster";

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * The guardian PUBLIC key last handed across the bridge — how we know whether
 * a provision would say anything new (S4).
 *
 * The pubkey, deliberately, never the secret: this is a module-scope variable
 * whose only job is change detection, and there is no version of that job that
 * needs a copy of the secret sitting in it.
 */
let provisionedPubkey: string | null = null;

/**
 * Hand the native shell the guardian key — ONCE per key, not once per render
 * (S4, review 2026-08-07).
 *
 * `App` calls this from an unconditional effect, deliberately, so a
 * restore-from-backup is picked up without a remount. That meant serialising
 * the guardian secret to hex and pushing it over the JS bridge on EVERY
 * render: dozens of copies of the family's root signing key strewn through JS
 * string heap, the bridge marshaller, and whatever the WebView logs — for a
 * value that changes perhaps twice in the life of an install.
 *
 * The effect stays unconditional (it is the honest way to catch a key change);
 * the hand-off is now gated on the key actually being different.
 *
 * Returns true when the carrier holds a current provision — including when
 * this call was a no-op because it already did. Callers use it as "is the
 * carrier provisioned", not as "did work happen".
 */
export function provisionCarrier(): boolean {
  const carrier = window.CharterCarrier;
  if (!carrier) return false;
  const loaded = readGuardianKey();
  if (loaded.kind !== "ok") return false;
  const pubkey = getPublicKey(loaded.key);
  if (pubkey === provisionedPubkey) return true;
  carrier.provision(
    JSON.stringify({
      v: 1,
      guardianSkHex: bytesToHex(loaded.key),
      guardianPubkeyHex: pubkey,
      relays: DEFAULT_RELAYS,
    }),
  );
  provisionedPubkey = pubkey;
  return true;
}

/** Test seam — forget which key we last provisioned, as a reload would. */
export function resetProvisionedForTests(): void {
  provisionedPubkey = null;
}

/**
 * Push the child/device roster so the carrier's notifications can name a
 * ward instead of saying "Your ward" (design memo 2026-08-04, part 1). A
 * CONVENIENCE, never a dependency: called from the same place and cadence as
 * `provisionCarrier` (every render, so it stays fresh whenever a child or
 * device changes — a device renamed after first provision must not go
 * stale), but its absence (outside the carrier, or an older shell without
 * the `roster` method) is always a silent no-op.
 */
export function pushCarrierRoster(children: Child[]): boolean {
  const carrier = window.CharterCarrier;
  if (!carrier?.roster) return false;
  carrier.roster(
    JSON.stringify({ v: 1, entries: buildCarrierRoster(children) }),
  );
  return true;
}

/** What this page can tell about the native shell it is running inside. */
export type CarrierShellState = "not-carrier" | "current" | "needs-update";

/**
 * Pure half of the staleness check — see [carrierNeedsUpdate].
 *
 * A shell without a `roster` method predates the ward-naming hand-off, so its
 * notifications still say "Your ward". `pushCarrierRoster` degrades to a
 * silent no-op there, which is correct — but silence is the wrong ANSWER for
 * a guardian, who then has no way to learn why the names never appeared
 * (decented, 2026-08-06: updated the PWA and the ward app, still saw "Your
 * ward", because the naming lives in Kotlin that only a new APK carries).
 *
 * Feature-detected, never version-compared: the page has no reliable read on
 * the shell's versionCode, and "does it expose the method I need" is the
 * honest question anyway.
 */
export function carrierShellState(
  carrier: { roster?: unknown } | undefined,
): CarrierShellState {
  if (!carrier) return "not-carrier";
  return carrier.roster ? "current" : "needs-update";
}

/** True when running inside a carrier shell too old to name wards. */
export function carrierNeedsUpdate(): boolean {
  return carrierShellState(window.CharterCarrier) === "needs-update";
}
