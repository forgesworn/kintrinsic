// A just-claimed phone has received NOTHING: pairing completed phone-side long
// ago, so no rule save has ever had it as a target. Until now the recovery was
// an instruction ("save their rules again") — this closes the loop by
// resending the child's standing charter automatically once the claim lands.
//
// The one subtlety is timing: `claimDevice` dispatches, but the claimed device
// only exists in COMMITTED state a render later — resolving the child target
// any earlier misses the very phone the resend is for (the same trap
// `confirmPairing` documents). So a claim parks a `PendingResend`, and the
// store's effect releases it only once the device is visible in state.

import type { Child } from "../domain/types";

/** A claim waiting for its device to appear in committed state. */
export interface PendingResend {
  childId: string;
  machine: string;
}

/** The pending resends whose claimed device is now committed — safe to send. */
export function readyResends(pending: PendingResend[], children: Child[]): PendingResend[] {
  return pending.filter((p) =>
    children.some(
      (c) =>
        c.id === p.childId &&
        c.devices.some((d) => d.pairing === "paired" && d.devicePubkey === p.machine),
    ),
  );
}
