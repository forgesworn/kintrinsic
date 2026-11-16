import type { OverrideScope } from "../wire/audit";

/** Where a press falls against the guardian's disconnect of the phone. */
export type DisconnectOrder = "before" | "after" | "unknown";

/** Set when the device had already been disconnected by the guardian. */
export interface ReleasedNote {
  /** The name the child had when the device was disconnected (the child may since have been removed). */
  childName: string;
  /**
   * Whether the press was made before or after the disconnect. "unknown" when
   * either time is unusable: no order is then claimed.
   */
  order: DisconnectOrder;
}

/**
 * The disconnect wording, in two parts: a clause that follows the device name,
 * and a sentence that follows the end of the line. Nothing for a device still
 * connected. The three forms mirror the carrier's alerts:
 *  - before: ", before you disconnected it from Mia";
 *  - after:  ", after you disconnected it from Mia" and "The phone may not have
 *    received the disconnect." (the press is the only sign the phone is still
 *    governed, so it is said plainly);
 *  - unknown: ", a phone you have disconnected", claiming no order.
 */
function disconnectWords(released?: ReleasedNote): { clause: string; tail: string } {
  if (!released) return { clause: "", tail: "" };
  const from = released.childName ? ` from ${released.childName}` : "";
  switch (released.order) {
    case "before":
      return { clause: `, before you disconnected it${from}`, tail: "" };
    case "after":
      return {
        clause: `, after you disconnected it${from}`,
        tail: "The phone may not have received the disconnect.",
      };
    default:
      return { clause: ", a phone you have disconnected", tail: "" };
  }
}

/**
 * The Activity sentence for an emergency unlock. The row already leads with
 * "Emergency unlock.", so this carries only what happened. It mirrors the
 * carrier's notification wording (`Notifier.overrideText`) so the alert and the
 * history never describe the same event differently.
 *
 * A missing scope or duration is left out rather than guessed: the line is a
 * record of what the ward's phone reported, and a wrong detail on an emergency
 * is worse than a missing one.
 */
export function overrideSummary(
  deviceLabel: string,
  scope: OverrideScope | null,
  durationSecs: number | null,
  released?: ReleasedNote,
): string {
  const what = scope === "full" ? "Opened their phone" : scope === "calls" ? "Opened calls" : "Used";
  const mins = durationSecs === null ? null : Math.max(1, Math.floor(durationSecs / 60));
  const forHowLong = mins === null ? "" : ` for ${mins} ${mins === 1 ? "minute" : "minutes"}`;
  const { clause, tail } = disconnectWords(released);
  return `${what} on ${deviceLabel}${forHowLong}${clause}${tail ? `. ${tail}` : ""}`;
}

/**
 * The Activity sentence for a use of the emergency-call button. The row leads
 * with "Emergency call button.". Deliberately honest about what is known: the
 * button was used. Kintrinsic cannot see whether the dialler appeared or a call
 * was made.
 */
export function emergencyCallSummary(deviceLabel: string, released?: ReleasedNote): string {
  const { clause, tail } = disconnectWords(released);
  return `Used the emergency call button on ${deviceLabel}${clause}. ${tail ? `${tail} ` : ""}Kintrinsic can't see whether a call was made.`;
}

/** The Activity sentence for an emergency control this build does not know by name. */
export function emergencyControlSummary(deviceLabel: string, released?: ReleasedNote): string {
  const { clause, tail } = disconnectWords(released);
  return `An emergency control was used on ${deviceLabel}${clause}.${tail ? ` ${tail}` : ""}`;
}
