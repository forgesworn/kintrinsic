import { deviceHealthNotes } from "../domain/deviceHealth";
import type { Device } from "../domain/types";
import type { LiveDeviceStatus } from "../store/store";

/**
 * The one short line that says a device has a problem, so it shows on the
 * child's page without opening the device; null when it has none. The device
 * page carries the full account of every one of these.
 */
export function deviceProblemLine(
  device: Device,
  status: LiveDeviceStatus | undefined,
  nowMs: number,
): string | null {
  if (device.pairing !== "paired") return "Disconnected. Open it to reconnect or remove it.";
  if (status?.updateHealth && status.updateHealth.attempts > 2) return "Its update isn’t completing.";
  if (status?.enforcementGap) return "It started up with Kintrinsic not running.";
  const { notes } = deviceHealthNotes(status, nowMs);
  if (notes.length > 0) return notes.length > 1 ? `${notes[0].text} (and ${notes.length - 1} more)` : notes[0].text;
  return null;
}
