// Fixtures shared by the Guardians screen tests: a family whose devices
// report guardians, and a mock of the store's guardian-keys API.

import { vi } from "vitest";
import type { Child, Device } from "../domain/types";
import { emptyGuardianKeys, type GuardianKeysState } from "../keys/state";
import type { StatusGuardians } from "../keys/statusGuardians";
import type { ChildReadiness } from "../keys/versionGate";

export const ME = "1".repeat(64);
export const OTHER = "2".repeat(64);
export const MY_W = "3".repeat(64);
export const OTHER_W = "4".repeat(64);
export const MY_OTHER_W = "5".repeat(64);
export const LAPTOP = "a".repeat(64);
export const PHONE = "b".repeat(64);
export const STATEMENT = "c".repeat(64);
export const TOKEN = "d".repeat(32);

export function guardians(over: Partial<StatusGuardians> = {}): StatusGuardians {
  return {
    roots: [
      { root: ME, keys: [MY_W, MY_OTHER_W] },
      { root: OTHER, keys: [OTHER_W] },
    ],
    pending: [],
    tombstones: 0,
    relays: ["wss://one.example"],
    ...over,
  };
}

export function device(label: string, machine: string, platform: Device["platform"], g?: StatusGuardians): Device {
  return {
    id: `dev-${machine.slice(0, 4)}`,
    label,
    platform,
    pairing: "paired",
    devicePubkey: machine,
    keysViewAt: 100,
    appVersionCode: platform === "android" ? 49 : 713,
    ...(g ? { guardians: g } : {}),
  };
}

export function family(devices: Device[]): Child[] {
  return [{ id: "c1", name: "Robin", color: "#abc", dependantPubkey: null, devices, policies: [] }];
}

const READY: ChildReadiness = { ready: true, wards: [] };

/** A store value with the guardian-keys API mocked; override what a test needs. */
export function mockStore(opts: {
  children: Child[];
  keys?: Partial<GuardianKeysState>;
  ready?: boolean;
  signer?: Record<string, unknown>;
}) {
  const keys: GuardianKeysState = { ...emptyGuardianKeys(), ...opts.keys };
  const machines = opts.children.flatMap((c) => c.devices.map((d) => d.devicePubkey as string));
  const notReady: ChildReadiness = {
    ready: false,
    wards: machines.map((m) => ({ machine: m, readiness: { ready: false, reason: "no-version" } as const })),
  };
  const childReadiness = opts.ready === false ? notReady : {
    ...READY,
    wards: machines.map((m) => ({ machine: m, readiness: { ready: true } as const })),
  };
  const report = { event: {}, sent: machines, held: [], failed: [] };
  return {
    state: {
      children: opts.children,
      signer: opts.signer ?? { connected: true, kind: "signet", autoSign: false, bunkerUri: "bunker://x" },
    },
    pairSignet: vi.fn(async () => ({ approved: true })),
    disconnectSigner: vi.fn(async () => {}),
    guardianKeys: {
      state: keys,
      workingPubkey: MY_W,
      rootPubkey: () => keys.root?.pubkey ?? MY_W,
      childReadiness: () => childReadiness,
      familyReadiness: () => ({
        ready: opts.ready !== false && opts.children.length > 0,
        children: opts.children.map((c) => ({ childId: c.id, readiness: childReadiness })),
      }),
      approveInstall: vi.fn(),
      inviteCoGuardian: vi.fn(async () => report),
      confirmPresence: vi.fn(async () => report),
      leaveFamily: vi.fn(async () => report),
      removeCoGuardian: vi.fn(async () => report),
      objectToRemoval: vi.fn(async () => report),
      cancelPending: vi.fn(async () => report),
      moveRoot: vi.fn(async () => report),
      moveProgress: vi.fn(() => null as null | { to: string; lapsed: boolean; expiresAt?: number; moved: string[] }),
      finishMove: vi.fn(async () => report),
      withdrawInstall: vi.fn(async () => report),
      setGuardianPin: vi.fn(async () => ({ sent: machines, held: [] as string[], failed: [] as { to: string; error: string }[] })),
      sendRoster: vi.fn(async () => ({ sent: [OTHER_W], held: [] as string[], failed: [] as { to: string; error: string }[] })),
      unconfirmedRosterDevices: vi.fn(() => [] as { subject: string; machine: string; name: string; platform: string }[]),
      anchorRoot: vi.fn((code: string) => code),
      trustedRoots: vi.fn(() => [keys.root?.pubkey ?? MY_W, ...Object.keys(keys.anchoredRoots ?? {})]),
      waitingPresenceWards: vi.fn(() => [] as { machine: string; confirmedAt: number; ts?: number }[]),
      movedAwayRoots: vi.fn(() => [] as string[]),
    },
  };
}

/** A My Signet root for this install, approval elided. */
export function signetRoot(): GuardianKeysState["root"] {
  return {
    pubkey: ME,
    via: "signet",
    approval: { id: "e".repeat(64), pubkey: ME, created_at: 1, kind: 31120, tags: [], content: "{}", sig: "f".repeat(128) },
  };
}

/** Set a controlled input's value the way React notices. */
export function typeInto(el: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement, value: string): void {
  const proto =
    el instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : el instanceof HTMLSelectElement
        ? HTMLSelectElement.prototype
        : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
  el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
}

export function button(container: HTMLElement, text: string): HTMLButtonElement | undefined {
  return [...container.querySelectorAll("button")].find((b) => b.textContent?.trim() === text);
}
