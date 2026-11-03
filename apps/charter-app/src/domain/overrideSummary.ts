import type { OverrideScope } from "../wire/audit";

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
): string {
  const what = scope === "full" ? "Opened their phone" : scope === "calls" ? "Opened calls" : "Used";
  const mins = durationSecs === null ? null : Math.max(1, Math.floor(durationSecs / 60));
  const forHowLong = mins === null ? "" : ` for ${mins} ${mins === 1 ? "minute" : "minutes"}`;
  return `${what} on ${deviceLabel}${forHowLong}`;
}
