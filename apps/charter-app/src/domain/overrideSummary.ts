import type { OverrideScope } from "../wire/audit";

/** Set when the device had already been disconnected by the guardian: the press was made before that. */
export interface ReleasedNote {
  /** The name the child had when the device was disconnected (the child may since have been removed). */
  childName: string;
}

/** ", before you disconnected it from Mia", or nothing for a device still connected. */
function beforeDisconnect(released?: ReleasedNote): string {
  if (!released) return "";
  return released.childName
    ? `, before you disconnected it from ${released.childName}`
    : ", before you disconnected it";
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
  return `${what} on ${deviceLabel}${forHowLong}${beforeDisconnect(released)}`;
}

/**
 * The Activity sentence for a use of the emergency-call button. The row leads
 * with "Emergency call button.". Deliberately honest about what is known: the
 * button was used. Kintrinsic cannot see whether the dialler appeared or a call
 * was made.
 */
export function emergencyCallSummary(deviceLabel: string, released?: ReleasedNote): string {
  return `Used the emergency call button on ${deviceLabel}${beforeDisconnect(released)}. Kintrinsic can't see whether a call was made.`;
}

/** The Activity sentence for an emergency control this build does not know by name. */
export function emergencyControlSummary(deviceLabel: string): string {
  return `An emergency control was used on ${deviceLabel}.`;
}
