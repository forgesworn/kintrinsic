// The versioning gate (contract, *Guardian roots and working keys (v1)*,
// *Versioning*): a ward that predates the section pins one guardian and
// refuses kind 31120, so for any child with such a ward this install keeps
// signing directly with its root. Working keys sign for a child, and
// co-guardians and root moves are offered, only once EVERY ward of that child
// reports an `appVersionCode` at or above the first version that implements
// the section.
//
// The two platforms number their builds differently (Linux sends
// major*10000 + minor*100 + patch of the package version; Android its own
// versionCode), so the threshold is per platform, keyed on the platform the
// device was paired as. See docs/guardian-keys-open-questions.md,
// "appVersionCode is per platform".

import type { Device } from "../domain/types";

/**
 * The first ward builds that implement *Guardian roots and working keys*.
 *
 * MUST be recorded in spec/contract.md (*Versioning*) when these builds are
 * released; until then they are the build's own numbers.
 */
export const GUARDIAN_KEYS_MIN_VERSION: Readonly<Record<Device["platform"], number>> = {
  android: 49,
  linux: 713,
};

export type WardReadiness =
  | { ready: true }
  /** No STATUS from it yet: fail closed, it may be an old ward. */
  | { ready: false; reason: "silent" }
  /** STATUS without `appVersionCode`: an old ward. */
  | { ready: false; reason: "no-version" }
  | { ready: false; reason: "too-old"; appVersionCode: number; needs: number }
  /** A platform this build has no threshold for. */
  | { ready: false; reason: "unknown-platform" };

/** Is one ward new enough for working keys? */
export function wardReadiness(
  platform: string | undefined,
  appVersionCode: number | undefined,
  statusSeen: boolean,
): WardReadiness {
  if (!statusSeen) return { ready: false, reason: "silent" };
  const needs =
    platform === "android" || platform === "linux" ? GUARDIAN_KEYS_MIN_VERSION[platform] : undefined;
  if (needs === undefined) return { ready: false, reason: "unknown-platform" };
  if (appVersionCode === undefined) return { ready: false, reason: "no-version" };
  if (appVersionCode < needs) return { ready: false, reason: "too-old", appVersionCode, needs };
  return { ready: true };
}

/** What the gate needs to know about one paired device. */
export interface GateDevice {
  machine: string;
  platform: string | undefined;
  appVersionCode?: number;
  /** A STATUS from it has been read by this install. */
  statusSeen: boolean;
}

export interface ChildReadiness {
  /** Every ward is ready (and there is at least one). */
  ready: boolean;
  wards: { machine: string; readiness: WardReadiness }[];
}

/**
 * A child is ready when every one of its paired wards is. A child with no
 * paired ward is not ready: there is nothing to show the gate is open.
 */
export function childReadiness(devices: readonly GateDevice[]): ChildReadiness {
  const wards = devices.map((d) => ({
    machine: d.machine,
    readiness: wardReadiness(d.platform, d.appVersionCode, d.statusSeen),
  }));
  return { ready: wards.length > 0 && wards.every((w) => w.readiness.ready), wards };
}
