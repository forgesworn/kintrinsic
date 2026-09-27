// Bridge the store's domain model to the signer's delivery target: a child's
// subject (dependant pubkey) + the pubkeys of every PAIRED device its clauses
// must reach + the relays to publish on. The store wires this as the
// RealSigner's `resolveChild` so screens keep calling `signClause(childId, …)`.

import type { Child } from "../domain/types";
import type { ChildTarget } from "./realSigner";
import { deviceRelays } from "./pairingRelays";

export function resolveChildTarget(
  children: Child[],
  childId: string,
  defaultRelays: string[],
): ChildTarget | undefined {
  const child = children.find((c) => c.id === childId);
  if (!child) return undefined;
  // Only REAL keys are deliverable. A non-hex devicePubkey (a demo `mockpub_…`
  // device, or a half-set-up one) must be SKIPPED — never fed to the gift-wrap,
  // where a bad hex recipient throws and aborts the whole sign, starving the
  // child's real devices too. (Caught by the full-UI e2e: a seed mock laptop
  // beside a real phone crashed every schedule save.)
  const paired = child.devices.filter(
    (d) =>
      d.pairing === "paired" &&
      typeof d.devicePubkey === "string" &&
      /^[0-9a-f]{64}$/.test(d.devicePubkey),
  );
  // The id rides along with the key so per-device rule overrides can be
  // resolved for the exact device each wrap is sealed to.
  const devices = paired.map((d) => ({ id: d.id, pubkey: d.devicePubkey as string }));
  // Each device's OWN relays (from its pairing), unioned with the defaults —
  // never just the current defaults, so a device paired under an older
  // default is not left unreachable when the defaults change.
  const relaySet = new Set(defaultRelays);
  for (const d of paired) {
    for (const r of deviceRelays(d, defaultRelays)) relaySet.add(r);
  }
  return { subject: child.dependantPubkey, name: child.name, devices, relays: [...relaySet] };
}
