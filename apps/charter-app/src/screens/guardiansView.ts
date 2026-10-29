// What the Guardians screens show, worked out from the household alone: who
// the family's devices trust (their STATUS `guardians`), what is waiting on
// each device, and which actions the contract or the versioning gate rules
// out, each with a one-line reason. Pure, so the screens stay thin and the
// gates are tested without a store.
//
// It mirrors the refusals in `keys/guardianKeys.ts` (which stay the real
// check): showing a disabled action with its reason beats a tap that fails.

import { nip19 } from "nostr-tools";
import type { Child, Device } from "../domain/types";
import type { GuardianKeysState } from "../keys/state";
import type { StatusGuardians } from "../keys/statusGuardians";
import type { WardReadiness } from "../keys/versionGate";
import { splitPendingD, type PendingType } from "../keys/sdk";
import { MIN_PIN_LENGTH } from "../keys/pin";

const HEX64 = /^[0-9a-f]{64}$/;

/** A device the keys layer delivers to: paired, a real device code, not departed. */
export function isKeysDevice(d: Device): d is Device & { devicePubkey: string } {
  return d.pairing === "paired" && d.departedAt === undefined && typeof d.devicePubkey === "string" && HEX64.test(d.devicePubkey);
}

export interface WardRef {
  childId: string;
  childName: string;
  device: Device & { devicePubkey: string };
}

/** Every deliverable device in the family, with its child. */
export function familyWards(children: readonly Child[]): WardRef[] {
  return children.flatMap((c) =>
    c.devices.filter(isKeysDevice).map((device) => ({ childId: c.id, childName: c.name, device })),
  );
}

/** Devices that told this phone it no longer has access to them. */
export function departedWards(children: readonly Child[]): { childName: string; device: Device }[] {
  return children.flatMap((c) =>
    c.devices.filter((d) => d.departedAt !== undefined).map((device) => ({ childName: c.name, device })),
  );
}

/** `npub1abcd…wxyz`: enough to tell two identities apart at a glance. */
export function shortNpub(hex: string): string {
  try {
    const n = nip19.npubEncode(hex);
    return `${n.slice(0, 12)}…${n.slice(-6)}`;
  } catch {
    return `${hex.slice(0, 8)}…`;
  }
}

/**
 * This install's root, without minting a key: the api's `rootPubkey()`
 * creates the local key when there is none, which merely opening Family
 * must not do. "" on a fresh install (nothing matches it).
 */
export function myRootOf(keys: { state: GuardianKeysState; workingPubkey: string | null }): string {
  return keys.state.root?.pubkey ?? keys.workingPubkey ?? "";
}

/** The full npub for a hex key. */
export function npubOf(hex: string): string {
  return nip19.npubEncode(hex);
}

/** A guardian identity typed or scanned, with any `nostr:` prefix a QR carries. */
export function stripNostrPrefix(input: string): string {
  return input.trim().replace(/^nostr:/i, "");
}

export interface RootView {
  root: string;
  /** This install's root. */
  you: boolean;
  /** Working keys the devices list under it, sorted. */
  keys: string[];
  /** Device names that pin it. */
  devices: string[];
}

/** The family's guardians as the devices report them, yours first. */
export function rootsView(wards: readonly WardRef[], me: string): RootView[] {
  const out = new Map<string, { keys: Set<string>; devices: string[] }>();
  for (const w of wards) {
    for (const r of w.device.guardians?.roots ?? []) {
      const e = out.get(r.root) ?? { keys: new Set<string>(), devices: [] };
      for (const k of r.keys) e.keys.add(k);
      e.devices.push(w.device.label);
      out.set(r.root, e);
    }
  }
  return [...out]
    .map(([root, e]) => ({ root, you: root === me, keys: [...e.keys].sort(), devices: e.devices }))
    .sort((a, b) => (a.you === b.you ? (a.root < b.root ? -1 : 1) : a.you ? -1 : 1));
}

export type PendingRole =
  /** This install sent it: it can cancel it. */
  | "mine"
  /** A removal of this install's root: it can object. */
  | "against-me"
  | "other";

export interface PendingView {
  d: string;
  kind: PendingType;
  /** The key the statement names (R2). */
  key: string;
  takesEffectAt?: number;
  expiresAt: number;
  role: PendingRole;
  /** The statement's event id, when this install knows it (it sent it). */
  statement?: string;
}

/** What is waiting on one device. */
export function pendingFor(g: StatusGuardians | undefined, keys: GuardianKeysState, me: string): PendingView[] {
  return (g?.pending ?? []).flatMap((p) => {
    const t = splitPendingD(p.d);
    if (!t) return [];
    const sent = keys.sent[p.d];
    const isMove = keys.nextRoot?.moveD === p.d;
    const role: PendingRole = sent || isMove ? "mine" : t.type === "removal" && t.key === me ? "against-me" : "other";
    const statement = sent?.id ?? (isMove ? keys.nextRoot?.moveId : undefined);
    return [
      {
        d: p.d,
        kind: p.kind,
        key: t.key,
        ...(p.takesEffectAt !== undefined ? { takesEffectAt: p.takesEffectAt } : {}),
        expiresAt: p.expiresAt,
        role,
        ...(statement ? { statement } : {}),
      },
    ];
  });
}

/** Pending removals of this install's root, merged across devices (for the objection banner). */
export function removalsAgainstMe(
  wards: readonly WardRef[],
  keys: GuardianKeysState,
  me: string,
): { d: string; devices: string[]; takesEffectAt?: number }[] {
  const out = new Map<string, { devices: string[]; takesEffectAt?: number }>();
  for (const w of wards) {
    for (const p of pendingFor(w.device.guardians, keys, me)) {
      if (p.role !== "against-me") continue;
      const e = out.get(p.d) ?? { devices: [] };
      e.devices.push(w.device.label);
      if (p.takesEffectAt !== undefined) {
        e.takesEffectAt = e.takesEffectAt === undefined ? p.takesEffectAt : Math.min(e.takesEffectAt, p.takesEffectAt);
      }
      out.set(p.d, e);
    }
  }
  return [...out].map(([d, e]) => ({ d, ...e }));
}

/** An action and, when it can't be offered, the one line saying why. */
export type Gate = { ok: true } | { ok: false; reason: string };

/** Mirrors `leaveFamily`: every reporting device must keep another guardian. */
export function leaveGate(wards: readonly WardRef[], me: string): Gate {
  const reporting = wards.filter((w) => w.device.guardians);
  if (reporting.length === 0) {
    return { ok: false, reason: "None of your devices has reported its guardians yet, so leaving can't be checked." };
  }
  const alone = reporting.filter((w) => !w.device.guardians!.roots.some((r) => r.root !== me));
  if (alone.length > 0) {
    return {
      ok: false,
      reason: `You're the only guardian on ${alone.map((w) => w.device.label).join(", ")}. Unpair it instead.`,
    };
  }
  return { ok: true };
}

/**
 * Removing the other parent needs this phone to confirm in person with a
 * working key bound to your identity, which a key held only on this phone
 * isn't (the ward refuses it as `confirmer_not_bound`).
 */
export function removeGate(keys: GuardianKeysState): Gate {
  if (!keys.root) {
    return {
      ok: false,
      reason:
        "Removing the other parent needs your guardian identity in My Signet or a Heartwood: a key kept only on this phone can't confirm in person.",
    };
  }
  return { ok: true };
}

/** Mirrors `moveRoot`: only a My Signet or Heartwood identity can move. */
export function moveGate(keys: GuardianKeysState, familyReady: boolean): Gate {
  if (!keys.root) {
    return {
      ok: false,
      reason: "Your guardian identity is kept on this phone. Moving it to another identity isn't available yet.",
    };
  }
  if (keys.nextRoot) return { ok: false, reason: "A move is already waiting to be confirmed at your devices." };
  if (!familyReady) return { ok: false, reason: "Every device needs the latest Kintrinsic first." };
  return { ok: true };
}

/** Android wards this install hasn't sent a guardian PIN to. */
export function androidWardsWithoutPin(wards: readonly WardRef[], keys: GuardianKeysState): WardRef[] {
  return wards.filter((w) => w.device.platform === "android" && keys.pinSentAt[w.device.devicePubkey] === undefined);
}

/** One plain line for a device that isn't ready for guardian keys. */
export function readinessLine(r: WardReadiness): string | null {
  if (r.ready) return null;
  switch (r.reason) {
    case "silent":
      return "Hasn't checked in with this phone yet. Open Kintrinsic on it, connected to the internet.";
    case "no-version":
    case "too-old":
      return "Needs a Kintrinsic update before it can take a second guardian.";
    case "unknown-platform":
      return "This kind of device can't take a second guardian yet.";
  }
}

/** "2 days 3 hours", "4 hours 10 minutes", "5 minutes", "less than a minute". */
export function durationWords(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  if (days > 0) return hours > 0 ? `${plural(days, "day")} ${plural(hours, "hour")}` : plural(days, "day");
  if (hours > 0) return minutes > 0 ? `${plural(hours, "hour")} ${plural(minutes, "minute")}` : plural(hours, "hour");
  if (minutes > 0) return plural(minutes, "minute");
  return "less than a minute";
}

/** What a pending item is, in a sentence fragment. */
export function pendingTitle(p: PendingView, me: string): string {
  const who = p.key === me ? "you" : shortNpub(p.key);
  switch (p.kind) {
    case "coguardian":
      return `Invitation for ${who}`;
    case "removal":
      return p.key === me ? "Removal of you" : `Removal of ${who}`;
    case "move":
      return `Identity move to ${who}`;
  }
}

/** Build the full presence code from its parts (the typed short form). */
export function presenceCodeFrom(machine: string, token: string, d: string, statement: string): string {
  const q = new URLSearchParams({ m: machine, t: token, d, s: statement });
  return `charter://presence?${q.toString()}`;
}

/** Why a guardian PIN draft can't be sent yet, or null when it can. */
export function pinProblem(pin: string, again: string): string | null {
  if (!/^[0-9]*$/.test(pin)) return "Use digits only.";
  if (pin.length < MIN_PIN_LENGTH) return `Choose a PIN of at least ${MIN_PIN_LENGTH} digits.`;
  if (pin !== again) return "The two PINs don't match.";
  return null;
}

/** Go to a Guardians sub-screen (`""` is the overview). */
export function goGuardians(sub = ""): void {
  window.location.hash = sub ? `/family/guardians/${sub}` : "/family/guardians";
}

/** The Guardians sub-route in `#/family/guardians/<sub>`, or null when not on Guardians. */
export function guardiansSubroute(hash = window.location.hash): string | null {
  const parts = hash.replace(/^#\/?/, "").split("/");
  if (parts[0] !== "family" || parts[1] !== "guardians") return null;
  return parts.slice(2).join("/");
}
