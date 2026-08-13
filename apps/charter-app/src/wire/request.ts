// Consuming a device REQUEST (kind 31111) — the reverse of what `charterd`
// publishes when a child asks for more time (`spec/contract.md` "Device
// brokering — time.extend"). Like STATUS, the REQUEST rumor is UNSIGNED on the
// wire (charterd publishes it bare); the machine's schnorr signature lives on
// the SEAL (kind 13), so authenticity = a valid machine-signed seal PLUS the
// author binding rumor.pubkey == seal.pubkey == payload.machine — the same
// binding charterd's own `nip59::unwrap` enforces in the other direction. The
// child-authored `reason` is private: it exists ONLY inside this E2E wrap.

import { nip44, verifyEvent, type NostrEvent } from "nostr-tools";
import { MAX_EXTEND_MINUTES } from "./grant";
import type { AppOpenRequestParams, TimeExtendLimitHit, TimeExtendRequestParams } from "./types";

export const CHARTER_DEVICE_REQUEST = 31111;
const SEAL = 13;
const MARKER: [string, string] = ["t", "charter-device"];

/** Contract bound for the child-authored `reason` (frozen; mirrors charter-proto). */
export const MAX_REASON_LEN = 280;

/**
 * Wrap freshness bound, seconds — mirrors `charter-transport::nip59::MAX_JITTER_SECS`
 * (2 days). NIP-59 wraps are randomly backdated up to this window, so the bound
 * is checked in BOTH directions: a wrap whose `created_at` is further than this
 * from now (stale relay replay, or a future-dated forgery) is dropped.
 */
export const MAX_WRAP_JITTER_SECS = 2 * 24 * 60 * 60;

/** Contract `RequestPayload`, discriminated by op. Kintrinsic ingests
 *  `time.extend` (the ask-for-more-time loop) and `install.apk` (a phone
 *  asking to install an app); `exec`/`flatpak` asks have no guardian screen
 *  yet and stay dropped at parse (fail-closed). */
interface DeviceRequestBase {
  v: 1;
  /** 32-byte hex — the id the GRANT must echo (and our dedupe key). */
  reqId: string;
  /** 32-byte hex — the single-use nonce the GRANT must echo. */
  nonce: string;
  /** The managed user's pubkey (hex) — who is asking. */
  subject: string;
  /** The machine's pubkey (hex) — which device brokered the ask. */
  machine: string;
  /** Unix SECONDS (device clock). */
  ts: number;
}

export interface TimeExtendDeviceRequest extends DeviceRequestBase {
  op: "time.extend";
  params: TimeExtendRequestParams;
}

export interface InstallApkDeviceRequest extends DeviceRequestBase {
  op: "install.apk";
  params: { packageName: string; label?: string; source: "staged" };
}

/** A child-authored ask to open (or keep open past a hold's end) an app the
 *  `apps` clause presently gates via `blocked`/`askFirst`. There is no typed
 *  GRANT for this op — the guardian's answer is a plain Decision plus,
 *  only on allow, a re-signed `apps` clause carrying an `AppHold`. */
export interface AppOpenDeviceRequest extends DeviceRequestBase {
  op: "app.open";
  params: AppOpenRequestParams;
}

export type DeviceRequest =
  | TimeExtendDeviceRequest
  | InstallApkDeviceRequest
  | AppOpenDeviceRequest;

/** Android package name: 2+ dot-separated segments, letter-led (proto parity). */
function validPackageName(s: unknown): s is string {
  if (typeof s !== "string" || s.length > 256) return false;
  const segs = s.split(".");
  return segs.length >= 2 && segs.every((seg) => /^[A-Za-z][A-Za-z0-9_]*$/.test(seg));
}

const LIMITS: TimeExtendLimitHit[] = ["schedule", "budget", "bucket"];

/** Bucket ids are `[a-z0-9-]`, ≤40 chars (mirrors `charter-schedule`'s
 *  `valid_bucket_id`) — a bound loose enough to accept anything the device
 *  could ever have minted, tight enough to refuse garbage. */
function validBucketId(s: unknown): s is string {
  return typeof s === "string" && /^[a-z0-9-]{1,40}$/.test(s);
}

function isHex(s: unknown): s is string {
  return typeof s === "string" && /^[0-9a-f]{64}$/.test(s);
}
function isNonNegInt(n: unknown): n is number {
  return typeof n === "number" && Number.isInteger(n) && n >= 0;
}

/**
 * Parse + validate a REQUEST payload from a rumor's `content` JSON. Returns
 * `null` on any malformed field — a bad ask is dropped, never trusted.
 * `minutesRequested` is bounds-checked (1..=1440) exactly like charter-proto.
 */
export function parseRequest(json: string): DeviceRequest | null {
  let o: Record<string, unknown>;
  try {
    o = JSON.parse(json) as Record<string, unknown>;
  } catch {
    return null;
  }
  if (o.v !== 1) return null;
  if (!isHex(o.reqId) || !isHex(o.nonce)) return null;
  if (!isHex(o.subject) || !isHex(o.machine)) return null;
  if (!isNonNegInt(o.ts)) return null;
  const p = o.params as Record<string, unknown> | undefined;
  if (!p || typeof p !== "object") return null;
  if (o.op === "install.apk") {
    if (!validPackageName(p.packageName)) return null;
    if (p.label !== undefined && (typeof p.label !== "string" || p.label.length > MAX_REASON_LEN)) {
      return null;
    }
    if (p.source !== "staged") return null;
    return {
      v: 1,
      op: "install.apk",
      reqId: o.reqId,
      nonce: o.nonce,
      subject: o.subject,
      machine: o.machine,
      ts: o.ts,
      params: {
        packageName: p.packageName,
        label: typeof p.label === "string" ? p.label : undefined,
        source: "staged",
      },
    };
  }
  if (o.op === "app.open") {
    if (typeof p.pkg !== "string" || p.pkg.trim().length === 0) return null;
    if (p.label !== undefined && (typeof p.label !== "string" || p.label.length > MAX_REASON_LEN)) {
      return null;
    }
    if (
      p.minutesRequested !== undefined &&
      (!isNonNegInt(p.minutesRequested) ||
        p.minutesRequested < 1 ||
        p.minutesRequested > MAX_EXTEND_MINUTES)
    ) {
      return null;
    }
    if (p.reason !== undefined && (typeof p.reason !== "string" || p.reason.length > MAX_REASON_LEN)) {
      return null;
    }
    return {
      v: 1,
      op: "app.open",
      reqId: o.reqId,
      nonce: o.nonce,
      subject: o.subject,
      machine: o.machine,
      ts: o.ts,
      params: {
        pkg: p.pkg,
        label: typeof p.label === "string" ? p.label : undefined,
        minutesRequested: typeof p.minutesRequested === "number" ? p.minutesRequested : undefined,
        reason: typeof p.reason === "string" ? p.reason : undefined,
      },
    };
  }
  if (o.op !== "time.extend") return null;
  if (
    !isNonNegInt(p.minutesRequested) ||
    p.minutesRequested < 1 ||
    p.minutesRequested > MAX_EXTEND_MINUTES
  ) {
    return null;
  }
  if (p.reason !== undefined && (typeof p.reason !== "string" || p.reason.length > MAX_REASON_LEN)) {
    return null;
  }
  if (!LIMITS.includes(p.limitHit as TimeExtendLimitHit)) return null;
  if (p.bucketId !== undefined && !validBucketId(p.bucketId)) return null;
  return {
    v: 1,
    op: "time.extend",
    reqId: o.reqId,
    nonce: o.nonce,
    subject: o.subject,
    machine: o.machine,
    ts: o.ts,
    params: {
      minutesRequested: p.minutesRequested,
      reason: typeof p.reason === "string" ? p.reason : undefined,
      limitHit: p.limitHit as TimeExtendLimitHit,
      bucketId: typeof p.bucketId === "string" ? p.bucketId : undefined,
    },
  };
}

/**
 * Unwrap a gift-wrapped (1059) REQUEST the device sealed to the guardian:
 * decrypt wrap → seal → rumor with the guardian secret key, authenticate the
 * MACHINE (the seal's schnorr sig — the only device signature on this wire —
 * plus rumor.pubkey == seal.pubkey == payload.machine, so a relay can't
 * attribute one device's ask to another), confirm the rumor is a marker-tagged
 * kind-31111 charter REQUEST, and parse it. Returns `null` on any decrypt /
 * verify / kind / freshness / parse failure (never throws).
 */
export function unwrapRequest(
  wrap: NostrEvent,
  recipientSk: Uint8Array,
  nowSecs: number = Math.floor(Date.now() / 1000),
): DeviceRequest | null {
  try {
    // Jitter: wraps are backdated; drop one too far from now in either direction
    // (the same window charterd's nip59::unwrap enforces on its side).
    if (Math.abs(nowSecs - wrap.created_at) > MAX_WRAP_JITTER_SECS) return null;
    const seal: NostrEvent = JSON.parse(
      nip44.decrypt(wrap.content, nip44.getConversationKey(recipientSk, wrap.pubkey)),
    );
    if (seal.kind !== SEAL) return null;
    // The seal is the machine's SIGNED event — verify it (id + schnorr sig).
    if (!verifyEvent(seal)) return null;
    const rumor: NostrEvent = JSON.parse(
      nip44.decrypt(seal.content, nip44.getConversationKey(recipientSk, seal.pubkey)),
    );
    if (rumor.kind !== CHARTER_DEVICE_REQUEST) return null;
    if (rumor.pubkey !== seal.pubkey) return null;
    if (!rumor.tags?.some((t) => t[0] === MARKER[0] && t[1] === MARKER[1])) return null;
    const request = parseRequest(rumor.content);
    if (request && request.machine !== rumor.pubkey) return null;
    return request;
  } catch {
    return null;
  }
}
