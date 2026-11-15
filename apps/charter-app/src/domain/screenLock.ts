// What the guardian is told about a ward phone's screen lock, and about a reset
// they sent: the state, whether a reset can be sent, and the plain-English
// outcome. Pure, so every state is testable without a screen.
//
// The ward reports the lock itself (`screenLockSecure`), whether it can reset
// it now (`screenLockResetReady`) and one outcome per reset id (`screenLockResets`,
// the last 16, newest first). The guardian keeps only the id and time of the
// reset it sent (`Device.screenLockReset`), never the PIN, and looks its id up.

import type { Device, DeviceScreenLockReset } from "./types";
import type { DeviceStatus } from "../wire/status";
import { wardenSupport } from "./wardenSupport";

/** How long a reset stays acceptable to the ward: it checks the signed time against an hour back. */
export const SCREEN_LOCK_WINDOW_SECS = 3600;

/** After this long the sent line is dropped; the outcome is in the ward's own history by then. */
const SENT_LINE_KEEP_SECS = 24 * 3600;

/** Beside the open-at-any-hour apps, for a device with no screen lock. */
export const NO_SCREEN_LOCK_WARNING =
  "Anyone holding this device can use its open-at-any-hour apps.";

export const WAITING_FOR_DEVICE = "Waiting for the device…";
/** A send the relays couldn't take, which this phone's carrier holds for the home network. */
export const HELD_FOR_HOME_NETWORK = "Held until the device is on your home network.";
/** An `accepted` with no final result an hour after the send: the device took
 *  it but may have restarted before it acted on it. */
export const ACCEPTED_UNCONFIRMED =
  "The device received it but didn't confirm the result. If nothing has changed, send it again.";
export const NO_RESPONSE_IN_AN_HOUR =
  "The device didn't respond within an hour. Try again when it's online.";

type ScreenLockStatus = Pick<
  DeviceStatus,
  "appVersionCode" | "screenLockSecure" | "screenLockResetReady" | "screenLockResets"
>;

/**
 * Whether to show the Screen lock card for a device at all. Hidden for a laptop,
 * for a ward that reports a Kintrinsic older than the reset (it would ignore the
 * command without a word), and for any device whose STATUS has no
 * `screenLockSecure` yet: without it we cannot say what the lock is, and a
 * button for a lock we know nothing about would be a guess.
 */
export function screenLockSupport(
  device: Device,
  deviceStatus: Record<string, DeviceStatus>,
): "offer" | "hidden" {
  if (device.pairing !== "paired" || !device.devicePubkey) return "hidden";
  const support = wardenSupport([device], deviceStatus, "screenLock");
  if (support.unsupported.length > 0 || support.tooOld.length > 0) return "hidden";
  const status = deviceStatus[device.devicePubkey];
  return typeof status?.screenLockSecure === "boolean" ? "offer" : "hidden";
}

/** Devices of a ward that report no screen lock, for the always-available warning. */
export function devicesWithoutScreenLock(
  devices: Device[],
  deviceStatus: Record<string, DeviceStatus>,
): Device[] {
  return devices.filter(
    (d) =>
      d.pairing === "paired" &&
      d.platform === "android" &&
      Boolean(d.devicePubkey) &&
      deviceStatus[d.devicePubkey as string]?.screenLockSecure === false,
  );
}

function clock(unix: number): string {
  const d = new Date(unix * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export interface ScreenLockProgress {
  tone: "waiting" | "ok" | "warn";
  text: string;
}

export interface ScreenLockReading {
  /** "Screen lock: on" or "No screen lock". */
  state: string;
  /** True when there is no screen lock, so the open-at-any-hour sentence applies. */
  noLock: boolean;
  /** False only when the ward says it cannot reset yet; a missing report is not "not ready". */
  ready: boolean;
  /** Why the button is disabled; null when it is not. */
  notReadyNote: string | null;
  /** The reset just sent: waiting, its outcome, or the one-hour expiry. Null when none is worth showing. */
  progress: ScreenLockProgress | null;
}

/** The plain-English words for the outcome the ward recorded for a reset. */
export function screenLockResultText(result: string, at: number, wardName: string): ScreenLockProgress {
  switch (result) {
    case "applied":
      return { tone: "ok", text: `Done at ${clock(at)}` };
    case "failed":
      return { tone: "warn", text: "The device couldn't change the screen lock. Try again." };
    case "not_ready":
      return {
        tone: "warn",
        text: `The device wasn't ready yet. It's ready after ${wardName} next unlocks it. Try again then.`,
      };
    case "notice_failed":
      return {
        tone: "warn",
        text: `The device couldn't tell ${wardName}, so it left the screen lock as it was. Try again.`,
      };
    case "rejected_time":
      return {
        tone: "warn",
        text: "The device turned it down: its clock and yours disagree, or it arrived more than an hour late.",
      };
    case "superseded":
      return { tone: "warn", text: "A newer reset replaced this one." };
    case "accepted":
      return {
        tone: "waiting",
        text: "The device received it but hasn't confirmed the result yet.",
      };
    default:
      return {
        tone: "warn",
        text: "The device sent a reply this version of Kintrinsic doesn't understand.",
      };
  }
}

/**
 * The card's whole state. `sent` is what this guardian last sent; the outcome
 * is the ward's own, looked up by id. A reset with no outcome yet is "waiting"
 * (or "held", when only this phone's carrier has it) for the hour the ward
 * would still accept it, then the plain "didn't respond". An `accepted` that
 * is still unconfirmed an hour after the send says so.
 */
export function screenLockReading(
  sent: DeviceScreenLockReset | undefined,
  status: ScreenLockStatus | undefined,
  nowUnix: number,
  wardName: string,
): ScreenLockReading {
  const secure = status?.screenLockSecure === true;
  const ready = status?.screenLockResetReady !== false;

  let progress: ScreenLockProgress | null = null;
  if (sent && nowUnix - sent.issuedAt < SENT_LINE_KEEP_SECS) {
    const entry = status?.screenLockResets?.find((r) => r.id === sent.id);
    const withinHour = nowUnix - sent.issuedAt < SCREEN_LOCK_WINDOW_SECS;
    if (entry) {
      progress =
        entry.result === "accepted" && !withinHour
          ? { tone: "warn", text: ACCEPTED_UNCONFIRMED }
          : screenLockResultText(entry.result, entry.at, wardName);
    } else if (withinHour) {
      progress = { tone: "waiting", text: sent.held ? HELD_FOR_HOME_NETWORK : WAITING_FOR_DEVICE };
    } else {
      progress = { tone: "warn", text: NO_RESPONSE_IN_AN_HOUR };
    }
  }

  return {
    state: secure ? "Screen lock: on" : "No screen lock",
    noLock: !secure,
    ready,
    notReadyNote: ready ? null : `Ready after ${wardName} next unlocks this device.`,
    progress,
  };
}

/** A PIN the ward will take: 4 to 16 digits and nothing else. */
export function isValidScreenLockPin(candidate: string): boolean {
  return /^[0-9]{4,16}$/.test(candidate);
}
