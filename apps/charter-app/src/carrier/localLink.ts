// Typed, feature-detected wrappers over the carrier's local-network bridge
// (`window.CharterCarrier.local*`; design: docs/local-network-design.md,
// "Carrier bridge").
//
// Every method on the bridge is OPTIONAL: an older APK lacks them, and a plain
// browser has no bridge at all. So every wrapper here
//   - checks `typeof carrier.x === "function"` first,
//   - calls it as `carrier.x(...)` on the object (never detached — Android's
//     addJavascriptInterface breaks if the receiver is lost),
//   - wraps the call in try/catch, and
//   - treats anything it cannot parse as "unavailable" (null) rather than
//     throwing or guessing.
// Callers never need their own try/catch, and a null answer always means "carry
// on over relays".

import type { NostrEvent } from "nostr-tools";

export type LocalSendState = "sent" | "queued" | "rejected" | "unavailable";

/** The result of a ward's signed ACK, as the bridge reports it. */
export const ACK_RESULTS = [
  "applied",
  "unchanged",
  "superseded",
  "expired",
  "refused",
  "failed",
  "ignored",
] as const;
export type AckResult = (typeof ACK_RESULTS)[number];

export type LocalDeliveryState =
  | "queued"
  | "sent"
  | "received"
  | "rejected"
  | AckResult;

const DELIVERY_STATES: readonly string[] = [
  "queued",
  "sent",
  "received",
  "rejected",
  ...ACK_RESULTS,
];

export interface LocalSession {
  machine: string;
  since: number;
}

export interface LocalInfo {
  available: boolean;
  listening: boolean;
  sessions: LocalSession[];
}

export interface LocalDeliveryEntry {
  state: LocalDeliveryState;
  at: number;
  machine: string;
}

export interface LocalInboxEntry {
  id: string;
  machine: string;
  wrap: NostrEvent;
  receivedAt: number;
}

const HEX64 = /^[0-9a-f]{64}$/;

function parse(json: unknown): unknown {
  if (typeof json !== "string") return null;
  try {
    return JSON.parse(json);
  } catch {
    return null;
  }
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Which of the local-network methods this page's shell actually exposes. */
export type LocalBridgeSupport = "none" | "partial" | "full";

/**
 * "none" = no bridge object at all (a plain browser), or a carrier that
 * predates local delivery; "partial" = some but not all methods (never
 * expected from a real APK, treated as unusable); "full" = all five.
 */
export function localBridgeSupport(): LocalBridgeSupport {
  const c = window.CharterCarrier;
  if (!c) return "none";
  const has = [c.localInfo, c.localSend, c.localDelivery, c.localInbox, c.localConsume].map(
    (f) => typeof f === "function",
  );
  if (has.every(Boolean)) return "full";
  return has.some(Boolean) ? "partial" : "none";
}

/** Why home-network delivery is not in use, for the words shown to a guardian. */
export type LocalRouteGap = "browser" | "old-carrier" | "not-running";

/** The current live view of the route. `null` gap = usable now. */
export function localInfo(): LocalInfo | null {
  const c = window.CharterCarrier;
  if (!c || typeof c.localInfo !== "function") return null;
  try {
    const raw = parse(c.localInfo());
    if (!isObject(raw)) return null;
    const sessions: LocalSession[] = [];
    if (Array.isArray(raw.sessions)) {
      for (const s of raw.sessions) {
        if (isObject(s) && typeof s.machine === "string" && HEX64.test(s.machine)) {
          sessions.push({ machine: s.machine, since: typeof s.since === "number" ? s.since : 0 });
        }
      }
    }
    return {
      available: raw.available === true,
      listening: raw.listening === true,
      sessions,
    };
  } catch {
    return null;
  }
}

/** True when the carrier can carry wraps over the home network right now. */
export function localRouteAvailable(): boolean {
  const c = window.CharterCarrier;
  if (!c || typeof c.localSend !== "function") return false;
  return localInfo()?.available === true;
}

/** Why the route is not in use (null when it is). */
export function localRouteGap(): LocalRouteGap | null {
  const c = window.CharterCarrier;
  if (!c) return "browser";
  if (localBridgeSupport() !== "full") return "old-carrier";
  return localInfo()?.available === true ? null : "not-running";
}

/** Hand one wrap to the carrier for direct delivery. null = bridge unusable. */
export function localSend(recipientHex: string, wrap: NostrEvent): LocalSendState | null {
  const c = window.CharterCarrier;
  if (!c || typeof c.localSend !== "function") return null;
  try {
    const raw = parse(c.localSend(recipientHex, JSON.stringify(wrap)));
    if (!isObject(raw)) return null;
    const s = raw.state;
    return s === "sent" || s === "queued" || s === "rejected" || s === "unavailable" ? s : null;
  } catch {
    return null;
  }
}

/** Delivery state per wrap id. Unknown ids / malformed entries are omitted. */
export function localDelivery(ids: string[]): Record<string, LocalDeliveryEntry> | null {
  const c = window.CharterCarrier;
  if (!c || typeof c.localDelivery !== "function") return null;
  try {
    const raw = parse(c.localDelivery(JSON.stringify(ids)));
    if (!isObject(raw)) return null;
    const out: Record<string, LocalDeliveryEntry> = {};
    for (const [id, v] of Object.entries(raw)) {
      if (!isObject(v) || typeof v.state !== "string" || !DELIVERY_STATES.includes(v.state)) continue;
      out[id] = {
        state: v.state as LocalDeliveryState,
        at: typeof v.at === "number" ? v.at : 0,
        machine: typeof v.machine === "string" ? v.machine : "",
      };
    }
    return out;
  } catch {
    return null;
  }
}

/** Wraps the carrier received from wards over the link. null = bridge unusable. */
export function localInbox(): LocalInboxEntry[] | null {
  const c = window.CharterCarrier;
  if (!c || typeof c.localInbox !== "function") return null;
  try {
    const raw = parse(c.localInbox());
    if (!Array.isArray(raw)) return null;
    const out: LocalInboxEntry[] = [];
    for (const e of raw) {
      if (!isObject(e) || typeof e.id !== "string" || !isObject(e.wrap)) continue;
      out.push({
        id: e.id,
        machine: typeof e.machine === "string" ? e.machine : "",
        wrap: e.wrap as unknown as NostrEvent,
        receivedAt: typeof e.receivedAt === "number" ? e.receivedAt : 0,
      });
    }
    return out;
  } catch {
    return null;
  }
}

/** Tell the carrier these inbox entries are safely ours. True if the call was made. */
export function localConsume(ids: string[]): boolean {
  const c = window.CharterCarrier;
  if (!c || typeof c.localConsume !== "function" || ids.length === 0) return false;
  try {
    c.localConsume(JSON.stringify(ids));
    return true;
  } catch {
    return false;
  }
}
