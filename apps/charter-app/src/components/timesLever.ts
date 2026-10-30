// What "More time" offers when a child has times: which kind of time, and which
// devices — the defaults, and the options the signer takes.
//
// Pure, so the rules a guardian relies on ("it picked the laptop that's locked",
// "it won't send Learning time to a phone that can't run it") are tested
// directly rather than only through a rendered sheet.

import { effectivePolicyForDevice } from "../domain/effectivePolicy";
import { canUseTimes, defaultExtendTime } from "../domain/times";
import type { Device, NamedTime, Policy } from "../domain/types";
import type { GiftOptions } from "../signer/Signer";
import type { DeviceStatus } from "../wire/status";
import { STATUS_FRESHNESS_SECS } from "../store/liveStatus";

/** The label for Free time in a picker: it has no entry in the child's `times`. */
export const FREE_TIME_LABEL = "Free time";

/** The kinds a gift can buy: Free time first, then each time the child has. */
export function timeChoices(times: readonly NamedTime[] | undefined): { id: string | undefined; label: string }[] {
  return [{ id: undefined, label: FREE_TIME_LABEL }, ...(times ?? []).map((t) => ({ id: t.id, label: t.label }))];
}

/** The devices a gift of this kind may go to: every paired, reachable device
 *  for Free time; only those that can run times for a typed gift (an older ward
 *  would land the minutes as Free time, the opposite of what was chosen). */
export function giftDevices(
  devices: readonly Device[],
  deviceStatus: Record<string, DeviceStatus>,
  typed: boolean,
): Device[] {
  return devices.filter(
    (d) => d.pairing === "paired" && !!d.devicePubkey && (!typed || canUseTimes(d, deviceStatus)),
  );
}

/**
 * The devices "More time" is aimed at until the guardian says otherwise: the
 * ones STATUS currently reports locked (they are the ones that need it), and
 * every candidate when none is — or when nothing has reported yet, since a
 * silent device is not evidence that it is unlocked.
 */
export function defaultGiftDeviceIds(
  candidates: readonly Device[],
  deviceStatus: Record<string, DeviceStatus>,
  nowMs: number,
  freshnessSecs: number = STATUS_FRESHNESS_SECS,
): string[] {
  const locked = candidates.filter((d) => {
    const s = d.devicePubkey ? deviceStatus[d.devicePubkey] : undefined;
    return !!s && nowMs / 1000 - s.ts <= freshnessSecs && s.locked;
  });
  return (locked.length > 0 ? locked : candidates).map((d) => d.id);
}

/**
 * Which kind the sheet opens on (`undefined` = Free): the contract's ward-side
 * rule, read off the first target device's own schedule — what it is in now,
 * else the time that most recently ended, else the next one today, else Free.
 */
export function defaultGiftTime(
  policy: Policy | undefined,
  targetDeviceId: string | undefined,
  nowMs: number,
): string | undefined {
  const times = policy?.times;
  if (!policy || !times || times.length === 0 || !targetDeviceId) return undefined;
  const schedule = effectivePolicyForDevice(policy, targetDeviceId).schedule;
  return defaultExtendTime(schedule, times, Math.floor(nowMs / 1000));
}

/**
 * What to hand `signGiftClause` after the guardian's choices. `undefined` when
 * the choice is the old behaviour (Free time, every paired device), so the call
 * is byte-for-byte the one made before times existed.
 */
export function giftOptions(args: {
  timeId: string | undefined;
  timeLabel: string | undefined;
  chosenIds: readonly string[];
  /** Every paired device the gift could reach for Free time. */
  allIds: readonly string[];
}): GiftOptions | undefined {
  const opts: GiftOptions = {};
  if (args.timeId) {
    opts.time = args.timeId;
    if (args.timeLabel) opts.timeLabel = args.timeLabel;
  }
  const everyone =
    args.chosenIds.length === args.allIds.length && args.allIds.every((id) => args.chosenIds.includes(id));
  if (!everyone) opts.deviceIds = [...args.chosenIds];
  return Object.keys(opts).length > 0 ? opts : undefined;
}
