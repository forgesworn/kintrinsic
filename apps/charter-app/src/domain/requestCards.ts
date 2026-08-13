// Pure mapping: a real device REQUEST (unwrapped by `wire/request.ts`) → the
// asks-inbox `ChildRequest` card `store.tsx` dispatches and Approvals.tsx
// renders. Split out of `store.tsx`'s `ingestDeviceRequest` so the mapping
// itself — which fields land where — is directly testable without a React
// provider. `id`/`nowMs` are injected (id generation and the wall clock are
// the caller's job) so the same input always produces the same card.

import type { ChildRequest, Device } from "./types";
import type { AppOpenDeviceRequest, TimeExtendDeviceRequest } from "../wire/request";
import { identityDisplayLabel } from "./launchSignatures";

/** A real `time.extend` REQUEST → its asks-inbox card. Carries the wire
 *  correlation (`reqId`/`nonce`/`machine`/`subject`) a GRANT must echo, and —
 *  named times — `bucketId` verbatim from the request, exactly like
 *  `limitHit` itself, so a bucket ask's card knows which group it's about. */
export function timeExtendRequestToCard(
  r: TimeExtendDeviceRequest,
  device: Device,
  childId: string,
  id: string,
  nowMs: number,
): ChildRequest {
  return {
    id,
    childId,
    requester: "device",
    deviceId: device.id,
    kind: "time.extend",
    // The device's clock stamps the ask; never date it into the future here.
    createdAt: Math.min(r.ts * 1000, nowMs),
    title: `${device.label} asks for ${r.params.minutesRequested} more minutes`,
    reason: r.params.reason,
    minutesRequested: r.params.minutesRequested,
    limitHit: r.params.limitHit,
    bucketId: r.params.bucketId,
    status: "pending",
    reqId: r.reqId,
    nonce: r.nonce,
    machine: r.machine,
    subject: r.subject,
  };
}

/** A real `app.open` REQUEST → its asks-inbox card. `appId` carries the raw
 *  on-device pkg identity (the same slot `install.app` uses for its
 *  packageName) and `appLabel` the display name, falling back to the pkg
 *  when the device sent none — resolved through `identityDisplayLabel` so a
 *  `cmdline:` identity (which should never itself be device-reported, but
 *  could ride back on an `askFirst` echo of an already-saved policy) never
 *  becomes this card's title/label verbatim. */
export function appOpenRequestToCard(
  r: AppOpenDeviceRequest,
  device: Device,
  childId: string,
  id: string,
  nowMs: number,
): ChildRequest {
  const label = identityDisplayLabel(r.params.label, r.params.pkg) ?? r.params.pkg;
  return {
    id,
    childId,
    requester: "device",
    deviceId: device.id,
    kind: "app.open",
    createdAt: Math.min(r.ts * 1000, nowMs),
    title: `${device.label} asks to open ${label}`,
    reason: r.params.reason,
    minutesRequested: r.params.minutesRequested,
    appLabel: label,
    appId: r.params.pkg,
    status: "pending",
    reqId: r.reqId,
    nonce: r.nonce,
    machine: r.machine,
    subject: r.subject,
  };
}
