// What the two daily levers (give time, stop now) need to know about a child,
// shared by the sheet that pulls them and the card that offers them.
//
// Moved out of GiveTime/FinishNow when those inline expanders gave way to
// LeverSheet. The rules are unchanged:
//
// - Giving time is offered unless NOTHING paired could apply it. A device that
//   simply hasn't reported (switched off, out of signal) is assumed capable:
//   the clause waits on the relay for it. Every platform is counted, on its
//   own version scheme, so a laptop is not silently exempt from the check.
// - Whether a stand-down STANDS is the DEVICE's answer, never a guess from
//   local state, and only a FRESH answer: a device that went dark mid-stand-
//   down must not pin "waiting on you" past the midnight lapse.

import { useMemo } from "react";
import { useCharter } from "../store/store";
import { STATUS_FRESHNESS_SECS } from "../store/liveStatus";
import { deliverableDevices, wardenSupport } from "../domain/wardenSupport";
import { standingStandDown } from "../domain/standdownGate";
import type { Child, Device } from "../domain/types";

/** Where the give stepper opens: the same opening ask a locked ward sends. */
export const DEFAULT_GIFT_MINUTES = 30;
export const GIFT_STEP = 5;
export const GIFT_MIN = 5;
export const GIFT_MAX = 240;

export function useGiveSupport(child: Child) {
  const { deviceStatus } = useCharter();
  return useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "gift"),
    [child.devices, deviceStatus],
  );
}

export interface StopState {
  canSend: boolean;
  /** Devices running a Kintrinsic too old to honour a stand-down, by name. */
  tooOld: Device[];
  /** True while a stand-down stands on a device, per its fresh report. */
  standing: boolean;
  /** Devices we have not heard from lately: they get it when they're back. */
  quiet: Device[];
}

export function useStopState(child: Child): StopState {
  const { deviceStatus } = useCharter();
  const devices = useMemo(() => deliverableDevices(child.devices), [child.devices]);
  const support = useMemo(
    () => wardenSupport(devices, deviceStatus, "standdown"),
    [devices, deviceStatus],
  );
  const now = Date.now();
  const standing = standingStandDown(devices, deviceStatus, now);
  const quiet = devices.filter((d) => {
    const s = deviceStatus[d.devicePubkey as string];
    return !s || now / 1000 - s.ts > STATUS_FRESHNESS_SECS;
  });
  return { canSend: support.canSend, tooOld: support.tooOld, standing, quiet };
}
