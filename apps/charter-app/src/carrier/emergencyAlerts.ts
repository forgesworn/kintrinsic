// Whether the phone will actually tell the guardian when a ward uses an
// emergency button.
//
// The carrier posts those alerts on its own "Emergencies" notification
// channel. The guardian can turn the app's notifications, or that channel,
// off in system settings, and the console cannot see that on its own. The
// shell can, through two OPTIONAL bridge methods (an older shell lacks them,
// and a browser tab has no bridge at all). Both degrade to "say nothing":
// a warning is only shown when the shell itself reports that alerts are off.
//
// Like every call across the bridge: feature-detected, called on the object
// (never detached), wrapped in try/catch, and an answer it cannot read counts
// as "don't know", never as "off".

export interface EmergencyAlertState {
  /** The app's notifications are enabled. */
  notifications: boolean;
  /** The Emergencies channel is enabled (its importance is not NONE). */
  channel: boolean;
}

/** The shell's answer, or null when it has none (a browser, an older shell, an unreadable reply). */
export function readEmergencyAlertState(): EmergencyAlertState | null {
  const carrier = window.CharterCarrier;
  if (typeof carrier?.emergencyAlertState !== "function") return null;
  try {
    const parsed: unknown = JSON.parse(carrier.emergencyAlertState());
    if (typeof parsed !== "object" || parsed === null) return null;
    const { notifications, channel } = parsed as Record<string, unknown>;
    if (typeof notifications !== "boolean" || typeof channel !== "boolean") return null;
    return { notifications, channel };
  } catch {
    return null;
  }
}

/** True only when the shell reports that emergency alerts will not reach the guardian. */
export function emergencyAlertsOff(state: EmergencyAlertState | null): boolean {
  return state !== null && (!state.notifications || !state.channel);
}

/** Open the system settings for emergency alerts. False when the shell cannot. */
export function openEmergencyAlertSettings(): boolean {
  const carrier = window.CharterCarrier;
  if (typeof carrier?.openEmergencyAlertSettings !== "function") return false;
  try {
    carrier.openEmergencyAlertSettings();
    return true;
  } catch {
    return false;
  }
}
