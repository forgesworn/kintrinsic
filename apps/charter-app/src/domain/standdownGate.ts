// Whether a stand-down currently STANDS, as the DEVICE reports it.
//
// (Version gating moved to ./wardenSupport, which every guardian action now
// shares — see the note there about why a quiet device must not be treated as
// an incapable one.)

import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";
import { STATUS_FRESHNESS_SECS } from "../store/liveStatus";

/**
 * Whether a stand-down stands — the device's answer, never a local guess, and
 * only a FRESH answer counts. A device that goes dark while stood down must
 * not pin the guardian's screen on "waiting on you" forever, including long
 * after the stand-down lapsed at the ward's midnight.
 */
export function standingStandDown(
  devices: Device[],
  deviceStatus: Record<string, DeviceStatus>,
  nowMs: number,
): boolean {
  return devices.some((d) => {
    const s = deviceStatus[d.devicePubkey as string];
    return (
      Boolean(s) &&
      nowMs / 1000 - (s as DeviceStatus).ts <= STATUS_FRESHNESS_SECS &&
      (s as DeviceStatus).lockReason === "standdown"
    );
  });
}
