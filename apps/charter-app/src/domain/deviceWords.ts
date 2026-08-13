// How Kintrinsic names a child's devices in guardian-facing copy.
//
// Kintrinsic started Linux-only, so the whole app says "computer". Once phones
// started pairing that quietly stopped being true: "these rules apply to all 2
// of Robin's computers" reads, to a parent, as two laptops — when one of them
// is the phone in his pocket. A phone is a computer to us and emphatically not
// to the family. Rules the parent can't correctly picture are rules they can't
// trust, so the noun follows the device.
//
// `Device.platform` has always carried this; the copy just never asked.

import type { Device } from "./types";

/** Typographic apostrophe — matches the rest of the app's copy. */
const APOS = "’";

export function possessive(name: string): string {
  return name.endsWith("s") ? `${name}${APOS}` : `${name}${APOS}s`;
}

export function deviceNoun(platform: Device["platform"], count: number): string {
  const one = platform === "android" ? "phone" : "computer";
  return count === 1 ? one : `${one}s`;
}

/**
 * The line under a child's rules saying what they cover. Deliberately says
 * "both their phone and computer" rather than a count when the kinds differ —
 * that is the sentence a parent can check against the things on the kitchen
 * table.
 */
export function deviceScopeNote(childName: string, devices: Device[]): string {
  const owner = possessive(childName);
  if (devices.length === 0) {
    return `These rules will apply once you pair a device for ${childName}.`;
  }
  if (devices.length === 1) {
    return `These rules cover ${devices[0].label}.`;
  }

  const phones = devices.filter((d) => d.platform === "android").length;
  const computers = devices.length - phones;

  // All of one kind: the plain plural is clearest.
  if (phones === 0 || computers === 0) {
    const noun = deviceNoun(devices[0].platform, devices.length);
    return devices.length === 2
      ? `These rules apply to both of ${owner} ${noun}.`
      : `These rules apply to all ${devices.length} of ${owner} ${noun}.`;
  }

  // Mixed. At two, name them both — no counting required of the reader.
  if (devices.length === 2) {
    return `These rules apply to both ${owner} phone and computer.`;
  }
  const parts = [
    phones > 0 ? `${phones} ${deviceNoun("android", phones)}` : null,
    computers > 0 ? `${computers} ${deviceNoun("linux", computers)}` : null,
  ].filter(Boolean);
  return `These rules apply to all ${devices.length} of ${owner} devices — ${parts.join(" and ")}.`;
}
