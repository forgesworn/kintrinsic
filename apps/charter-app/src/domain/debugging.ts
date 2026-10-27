import type { DeviceStatus } from "../wire/status";
import type { Device, DeviceDebugging } from "./types";
import { wardenSupport } from "./wardenSupport";

// The guardian's USB-debugging toggle for ONE ward device. The wire is the
// `debugging` clause (spec/contract.md, "Debugging"); this is the display side:
// what the last thing we sent means right now.

/** The plain warning shown before USB debugging is turned on. */
export const DEBUGGING_WARNING =
  "While USB debugging is on, someone with a computer and the know-how can stop Kintrinsic on this device or read what’s on it. Turn it on only for testing or fixing a problem.";

export const DEBUGGING_HINT = "On the device, turn on USB debugging in Developer options.";

export type DebuggingView =
  | { state: "off" }
  | { state: "on"; until?: number };

/**
 * What the last-sent record means at `nowUnix`. Mirrors the ward's own rule:
 * no record / `allow: false` / `now >= until` is blocked, so an allowance that
 * has run out reads Off here exactly as it does on the device.
 */
export function debuggingView(
  sent: DeviceDebugging | undefined,
  nowUnix: number,
): DebuggingView {
  if (!sent || !sent.allow) return { state: "off" };
  if (sent.until != null && nowUnix >= sent.until) return { state: "off" };
  return sent.until != null ? { state: "on", until: sent.until } : { state: "on" };
}

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

function clock(d: Date): string {
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** "Off", "On until 18:00" (a weekday is added when it ends on another day), or "On until you turn it off". */
export function debuggingLabel(sent: DeviceDebugging | undefined, nowUnix: number): string {
  const v = debuggingView(sent, nowUnix);
  if (v.state === "off") return "Off";
  if (v.until == null) return "On until you turn it off";
  const end = new Date(v.until * 1000);
  const today = new Date(nowUnix * 1000);
  const sameDay = end.toDateString() === today.toDateString();
  return `On until ${sameDay ? "" : `${DAYS[end.getDay()]} `}${clock(end)}`;
}

/**
 * How to treat the toggle for this device: `offer` (Android, and not reporting a
 * Kintrinsic older than the first build that knows the clause), `tooOld` (an
 * Android ward that reports an older one: say so), or `none` (Linux, or not
 * paired: show nothing). Silence is not incapacity, so an Android ward that has
 * not reported yet is offered the toggle; the clause waits for it.
 */
export function debuggingSupport(
  device: Device,
  deviceStatus: Record<string, DeviceStatus>,
): "offer" | "tooOld" | "none" {
  if (device.pairing !== "paired" || !device.devicePubkey) return "none";
  const support = wardenSupport([device], deviceStatus, "debugging");
  if (support.unsupported.length > 0) return "none";
  return support.tooOld.length > 0 ? "tooOld" : "offer";
}
