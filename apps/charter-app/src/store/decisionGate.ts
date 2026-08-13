import type { AppOpenDecisionWire, DecisionWire, InstallDecision } from "../signer/Signer";

/**
 * Which path an approve/deny takes given the signer's connection state.
 *
 *  - "sign"           → hand the decision to the signer (it publishes the
 *                        GRANT when the ask is wire-correlated).
 *  - "record-locally" → offline draft: no signer connected and nothing on the
 *                        wire is owed an answer (simulated/demo asks only).
 *
 * A wire-correlated ask (a REAL device brokered it and is waiting on a signed
 * GRANT) must NEVER silently succeed while disconnected — that reads as
 * "approved" on the phone while the device stays locked forever.
 */
export function decisionPath(
  connected: boolean,
  wire: DecisionWire | InstallDecision | AppOpenDecisionWire | undefined,
): "sign" | "record-locally" {
  if (connected) return "sign";
  if (wire) {
    throw new Error(
      "This ask came from a device and needs your signature to answer — turn on parent approval, then decide again.",
    );
  }
  return "record-locally";
}
