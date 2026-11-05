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
import { PENDING_LIFETIME_SECONDS, removerOn } from "../keys/guardianKeys";
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
 * creates the local key when there is none, which merely opening Settings
 * must not do. "" on a fresh install (nothing matches it).
 */
export function myRootOf(keys: { state: GuardianKeysState; workingPubkey: string | null; rootPubkey?: () => string }): string {
  if (keys.state.root) return keys.state.root.pubkey;
  // With no root recorded, the phone's own key is the identity (never a W it minted).
  try {
    if (keys.rootPubkey) return keys.rootPubkey();
  } catch {
    // The local key can't be read: fall through.
  }
  return keys.workingPubkey ?? "";
}

/**
 * Said before a phone-held root first mints its separate working key
 * (decision D5): inviting, removing, confirming in person or moving.
 */
export const MINT_NOTICE =
  "This phone will now use a separate day-to-day key, vouched for by your identity; your identity itself is only used to add or remove parents and to move.";

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
  /**
   * The statement's event id, what a confirmation names: as the device
   * reports it, or, from a device that doesn't report ids, when this install
   * sent it.
   */
  statement?: string;
  /** The statement's event id as the device reports it (newer devices). */
  id?: string;
}

/**
 * What is waiting on one device: what its latest STATUS lists, less anything
 * past its `expiresAt` at `now` (unix s), which the device has dropped too.
 */
export function pendingFor(g: StatusGuardians | undefined, keys: GuardianKeysState, me: string, now: number): PendingView[] {
  return (g?.pending ?? []).flatMap((p) => {
    const t = splitPendingD(p.d);
    if (!t || p.expiresAt <= now) return [];
    // A removal of this install's root is never its own pending item: one
    // it signed itself (leaving) applies at once. The same `d` can hold a
    // past self-removal in `sent` and now someone else's removal of it.
    const againstMe = t.type === "removal" && t.key === me;
    // What this install sent under this `d` is the pending item only if it
    // is the same type and could still be pending: a statement newer than
    // it (another signer's, or after a rejoin) expires later than it can.
    // With the device's id it is exact; without one, by age.
    const own = keys.sent[p.d];
    const sent =
      !againstMe &&
      own &&
      own.type === t.type &&
      (p.id !== undefined ? own.id === p.id : p.expiresAt <= own.issuedAt + PENDING_LIFETIME_SECONDS + 300)
        ? own
        : undefined;
    const isMove = !againstMe && keys.nextRoot?.moveD === p.d && (p.id === undefined || keys.nextRoot.moveId === p.id);
    const role: PendingRole = againstMe ? "against-me" : sent || isMove ? "mine" : "other";
    const statement = p.id ?? sent?.id ?? (isMove ? keys.nextRoot?.moveId : undefined);
    return [
      {
        d: p.d,
        kind: p.kind,
        key: t.key,
        ...(p.takesEffectAt !== undefined ? { takesEffectAt: p.takesEffectAt } : {}),
        expiresAt: p.expiresAt,
        role,
        ...(statement ? { statement } : {}),
        ...(p.id ? { id: p.id } : {}),
      },
    ];
  });
}

/** A pending removal of this install's root, merged across the devices that list it. */
export interface RemovalAgainstMe {
  d: string;
  devices: string[];
  /** The children those devices belong to, each once. */
  children: string[];
  takesEffectAt?: number;
  /** The parent who asked, when every device listing it names the same one (the one other guardian there). */
  remover?: string;
  /** This phone has objected at every device listing it (decision Q1). */
  objected: boolean;
}

/** Pending removals of this install's root, merged across devices (for the removal notice). */
export function removalsAgainstMe(
  wards: readonly WardRef[],
  keys: GuardianKeysState,
  me: string,
  now: number,
): RemovalAgainstMe[] {
  const out = new Map<string, RemovalAgainstMe & { removers: Set<string | null> }>();
  for (const w of wards) {
    for (const p of pendingFor(w.device.guardians, keys, me, now)) {
      if (p.role !== "against-me") continue;
      const e = out.get(p.d) ?? { d: p.d, devices: [], children: [], objected: true, removers: new Set<string | null>() };
      e.devices.push(w.device.label);
      if (!e.children.includes(w.childName)) e.children.push(w.childName);
      e.removers.add(removerOn(w.device.guardians!, me));
      if (!(keys.objected?.[w.device.devicePubkey] ?? []).includes(p.id ?? p.d)) e.objected = false;
      if (p.takesEffectAt !== undefined) {
        e.takesEffectAt = e.takesEffectAt === undefined ? p.takesEffectAt : Math.min(e.takesEffectAt, p.takesEffectAt);
      }
      out.set(p.d, e);
    }
  }
  return [...out.values()].map(({ removers, ...e }) => {
    const only = removers.size === 1 ? [...removers][0] : null;
    return { ...e, ...(only ? { remover: only } : {}) };
  });
}

/** What to call another parent: the name this phone knows them by, else a short npub. */
export function parentName(root: string, keys: GuardianKeysState): string {
  return keys.anchoredRoots?.[root]?.label ?? shortNpub(root);
}

/** "Robin", "Robin and Sam", "Robin, Sam and Alex". */
export function namesList(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** An action and, when it can't be offered, the one line saying why. */
export type Gate = { ok: true } | { ok: false; reason: string };

/** Has this install read a STATUS from the device? */
export function heardFrom(d: Device): boolean {
  return d.keysViewAt !== undefined || d.lastSeenAt !== undefined;
}

/**
 * A device the other guardian's roster named that has never reported to
 * this phone: nothing shows this phone's guardian is pinned there yet.
 */
export function isUnconfirmed(d: Device): boolean {
  return !!d.fromRoster && !heardFrom(d);
}

/**
 * Mirrors `leaveFamily`: every device this phone paired or has heard from
 * must report its guardians, and keep another guardian. A device known only
 * from the roster and never heard from isn't counted.
 */
export function leaveGate(wards: readonly WardRef[], me: string, opts: { waitingForRoster?: boolean } = {}): Gate {
  const own = wards.filter((w) => !isUnconfirmed(w.device));
  if (own.length === 0) {
    return {
      ok: false,
      reason: opts.waitingForRoster
        ? "The other parent's phone hasn't shared the family with this one yet. Open Kintrinsic on their phone, then try again."
        : "None of your devices has reported its guardians yet, so leaving can't be checked.",
    };
  }
  const silent = own.filter((w) => !w.device.guardians);
  if (silent.length > 0) {
    const names = silent.map((w) => w.device.label).join(", ");
    return {
      ok: false,
      reason: `${names} ${silent.length === 1 ? "hasn't" : "haven't"} reported ${silent.length === 1 ? "its" : "their"} guardians yet, so leaving can't be checked.`,
    };
  }
  const alone = own.filter((w) => !w.device.guardians!.roots.some((r) => r.root !== me));
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
 * working key bound to your identity. A phone-held identity mints one when
 * the removal starts (decision D5), so it is always offered.
 */
export function removeGate(keys: GuardianKeysState): Gate {
  void keys;
  return { ok: true };
}

/**
 * What the typed short presence code can confirm (decision D10): every
 * pending statement the devices report with its event id, less what this
 * phone can't confirm. That is: an invitation of this phone's own identity,
 * a move to the identity it is moving to, and a removal it started itself
 * (only the remover's phones may confirm one). A device that doesn't report
 * ids still offers what this phone sent. `confirmPresence` checks it all again.
 */
export function typedPresenceItems(
  wards: readonly WardRef[],
  keys: GuardianKeysState,
  me: string,
  now: number,
): (PendingView & { machine: string; device: string; statement: string })[] {
  return wards.flatMap((w) =>
    pendingFor(w.device.guardians, keys, me, now)
      .filter(
        (p): p is PendingView & { statement: string } =>
          p.statement !== undefined &&
          (p.kind === "coguardian"
            ? p.key === me
            : p.kind === "move"
              ? p.key === keys.nextRoot?.pubkey
              : p.role === "mine"),
      )
      .map((p) => ({ ...p, machine: w.device.devicePubkey, device: w.device.label })),
  );
}

/** What `moveProgress()` says about a move in progress. */
export interface MoveProgress {
  to: string;
  lapsed: boolean;
  expiresAt?: number;
  /** Devices that already switched to the new identity. */
  moved: string[];
}

/** A move that lapsed after some devices switched: only finishing it gets the family unstuck. */
export function moveNeedsFinishing(progress: MoveProgress | null | undefined): boolean {
  return !!progress && progress.lapsed && progress.moved.length > 0;
}

/**
 * Mirrors `moveRoot`: only a My Signet or Heartwood identity can move, one
 * move at a time. A lapsed move is offered again: to finish, when some
 * devices switched, or to start afresh.
 */
export function moveGate(keys: GuardianKeysState, familyReady: boolean, progress?: MoveProgress | null): Gate {
  if (!keys.root) {
    return {
      ok: false,
      reason: "Your guardian identity is kept on this phone. Moving it to another identity isn't available yet.",
    };
  }
  // A lapsed move is either finished (some devices switched) or started
  // afresh (none did): `moveRoot` allows both.
  if (keys.nextRoot && !progress?.lapsed) {
    return { ok: false, reason: "A move is already waiting to be confirmed at your devices." };
  }
  if (!familyReady) return { ok: false, reason: "Every device needs the latest Kintrinsic first." };
  return { ok: true };
}

/**
 * Where an Android ward's guardian PIN stands (decision D4):
 * - "set": its STATUS says it holds one (whoever set it);
 * - "sending": this install sent one and the ward hasn't said it holds it yet;
 * - "none": it says it has none, or has never said.
 */
export function pinStateOf(w: WardRef, keys: GuardianKeysState): "set" | "sending" | "none" {
  if (w.device.guardians?.pinSet === true) return "set";
  return keys.pins[w.device.devicePubkey]?.state === "sending" ? "sending" : "none";
}

/**
 * Android wards that need a guardian PIN before anything that needs presence:
 * their STATUS says they have none, or has never said. One this install is
 * already sending a PIN to isn't asked again (a second PIN would replace it).
 */
export function androidWardsWithoutPin(wards: readonly WardRef[], keys: GuardianKeysState): WardRef[] {
  return wards.filter((w) => w.device.platform === "android" && pinStateOf(w, keys) === "none");
}

/** Android wards this install is still setting a guardian PIN on. */
export function androidWardsSettingPin(wards: readonly WardRef[], keys: GuardianKeysState): WardRef[] {
  return wards.filter((w) => w.device.platform === "android" && pinStateOf(w, keys) === "sending");
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
  window.location.hash = sub ? `/settings/guardians/${sub}` : "/settings/guardians";
}

/** The Guardians sub-route in `#/settings/guardians/<sub>`, or null when not on Guardians. */
export function guardiansSubroute(hash = window.location.hash): string | null {
  const parts = hash.replace(/^#\/?/, "").split("/");
  if (parts[0] !== "settings" || parts[1] !== "guardians") return null;
  return parts.slice(2).join("/");
}
