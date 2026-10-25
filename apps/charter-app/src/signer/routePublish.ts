// One shared publish seam for the two real signers (local key and Signet):
// relay AND home-network delivery, started together, whichever lands first.
//
// Design: docs/local-network-design.md, "Routing and fallback". In short, for
// a gift wrap addressed to one ward device, when the Android carrier can reach
// wards directly:
//
//   1. start the relay publish and hand the wrap to the carrier at the same
//      time (the SAME event — same id — so the ward collapses the two copies);
//   2. if the carrier wrote it to a live session, wait up to 3 s for the ward's
//      receipt. Receipt (or any later ACK) completes the publish without
//      waiting for relays; the relay publish carries on in the background;
//   3. otherwise the relay result decides, exactly as before;
//   4. if the relays failed but the carrier is HOLDING the wrap for a ward
//      that is not connected, say so — "not sent yet" — which is neither
//      success nor a hard failure (QueuedForHomeNetworkError).
//
// With no usable local route (a browser, an older APK, no Wi-Fi) this is the
// relay publish and nothing else, byte for byte what it was before.

import type { NostrEvent } from "nostr-tools";
import {
  localDelivery,
  localRouteAvailable,
  localSend,
  type LocalDeliveryState,
} from "../carrier/localLink";

/** How long to wait for the ward's receipt once the carrier has sent. */
export const LOCAL_RECEIPT_WINDOW_MS = 3_000;
/** How often to ask the carrier whether the receipt has arrived. */
export const LOCAL_POLL_MS = 150;

/** What kind of wrap this is, for delivery tracking. Everything but a GRANT or
 *  a rule/one-off CLAUSE is "other" and is not tracked. */
export interface PublishMeta {
  kind: "grant" | "clause" | "other";
  /** The ask a grant answers. */
  reqId?: string;
}

/** A wrap the carrier accepted (sent, or held for a ward that isn't connected). */
export interface RoutedWrap {
  wrapId: string;
  /** The recipient ward device's machine key. */
  machine: string;
  /** The carrier's state at hand-over. */
  state: "sent" | "queued";
  meta: PublishMeta;
}

export interface LocalRouteHooks {
  /** A guardian-facing name for a ward device, from its machine key. */
  deviceName?: (machine: string) => string | undefined;
  /** Called when the carrier accepted a wrap, so delivery can be tracked. */
  onRouted?: (wrap: RoutedWrap) => void;
}

/**
 * The relays failed, but the carrier is holding the change for a ward device
 * that is not on the home network right now; it goes out as soon as it is.
 * Callers show this as a WARNING and must not treat it as success — an ask
 * answered this way stays pending.
 */
export class QueuedForHomeNetworkError extends Error {
  readonly devices: string[];
  constructor(devices: string[]) {
    const names = [...new Set(devices.length > 0 ? devices : ["their device"])];
    super(QueuedForHomeNetworkError.describe(names));
    this.name = "QueuedForHomeNetworkError";
    this.devices = names;
  }

  private static describe(names: string[]): string {
    if (names.length === 1) {
      return `Not sent yet — it'll reach ${names[0]} when it's on the home network.`;
    }
    const last = names[names.length - 1];
    return `Not sent yet — it'll reach ${names.slice(0, -1).join(", ")} and ${last} when they're on the home network.`;
  }

  /** One error naming every device across several queued wraps. */
  static merge(errors: QueuedForHomeNetworkError[]): QueuedForHomeNetworkError {
    return new QueuedForHomeNetworkError(errors.flatMap((e) => e.devices));
  }
}

export const NO_RELAY_MESSAGE = "Couldn't reach any relay — check your connection and try again.";

type RelayPublish = (relays: string[], event: NostrEvent) => Promise<void>;
export type RoutedPublish = (
  relays: string[],
  event: NostrEvent,
  meta?: PublishMeta,
) => Promise<void>;

/** The single `p`-tag recipient of a kind-1059 wrap, else null. */
export function singleRecipient(event: NostrEvent): string | null {
  if (event.kind !== 1059) return null;
  const p = event.tags.filter((t) => t[0] === "p");
  if (p.length !== 1) return null;
  return /^[0-9a-f]{64}$/.test(p[0][1] ?? "") ? p[0][1] : null;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function isReceipt(state: LocalDeliveryState | undefined): boolean {
  // received, or any ward ACK. `rejected` is the carrier refusing the wrap;
  // queued/sent mean no receipt yet.
  return state !== undefined && state !== "queued" && state !== "sent" && state !== "rejected";
}

/**
 * Wrap a relay-only publish with the home-network route.
 *
 * Behaviour is unchanged unless the carrier reports an available local route
 * AND the event is a gift wrap with exactly one recipient.
 */
export function withLocalRoute(relayPublish: RelayPublish, hooks: LocalRouteHooks = {}): RoutedPublish {
  return async (relays, event, meta = { kind: "other" }) => {
    const recipient = singleRecipient(event);
    if (!recipient || !localRouteAvailable()) return relayPublish(relays, event);

    // Both routes start now. The relay promise is observed here, once, so a
    // rejection after the local route has already succeeded can never surface
    // as an unhandled rejection; the race below re-uses the same promise.
    const relay: { state: "pending" | "ok" | "failed"; error?: unknown } = { state: "pending" };
    const relayRun: Promise<void> =
      relays.length === 0
        ? Promise.reject(new Error(NO_RELAY_MESSAGE))
        : (async () => relayPublish(relays, event))(); // starts synchronously
    const relayDone = relayRun.then(
      () => {
        relay.state = "ok";
      },
      (err: unknown) => {
        relay.state = "failed";
        relay.error = err;
      },
    );

    const sent = localSend(recipient, event);
    let lastLocal: LocalDeliveryState | undefined =
      sent === "sent" || sent === "queued" ? sent : undefined;
    if (lastLocal !== undefined) {
      try {
        hooks.onRouted?.({ wrapId: event.id, machine: recipient, state: lastLocal, meta });
      } catch {
        // Tracking is bookkeeping; it must never fail a delivery.
      }
    }

    if (sent === "sent") {
      const deadline = Date.now() + LOCAL_RECEIPT_WINDOW_MS;
      for (;;) {
        const entry = localDelivery([event.id])?.[event.id];
        if (entry) lastLocal = entry.state;
        if (isReceipt(lastLocal)) return; // the ward has it; relays carry on unobserved
        if (lastLocal === "rejected") break;
        // A relay that already took it is as good as a receipt for the caller —
        // no reason to hold the guardian for the rest of the window.
        if (relay.state === "ok") return;
        if (Date.now() >= deadline) break;
        // Wake early if the relay settles meanwhile; once it has settled,
        // only the clock is worth waiting for.
        await (relay.state === "pending"
          ? Promise.race([sleep(LOCAL_POLL_MS), relayDone])
          : sleep(LOCAL_POLL_MS));
      }
    }

    await relayDone;
    if (relay.state === "ok") return;

    // Relays failed. One last look: the receipt may have landed while we waited.
    if (lastLocal === "sent" || lastLocal === "queued") {
      const entry = localDelivery([event.id])?.[event.id];
      if (entry) lastLocal = entry.state;
    }
    if (isReceipt(lastLocal)) return;
    if (lastLocal === "queued") {
      throw new QueuedForHomeNetworkError([hooks.deviceName?.(recipient) ?? "their device"]);
    }
    throw relay.error;
  };
}
