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
//   4. if the relays failed but the carrier is HOLDING the wrap — queued for a
//      ward that is not connected, or sent with no receipt yet — say so: "not
//      sent yet", which is neither success nor a hard failure
//      (QueuedForHomeNetworkError). The carrier keeps every wrap until the
//      ward's OK, so a held wrap WILL arrive; reporting it as a failure would
//      invite a retry that signs a second gift or a conflicting answer.
//
// The publish resolves with what the home network saw (`PublishOutcome`), so
// a caller can tell a ward's receipt from a ward's refusal: any ACK ends the
// wait, but only some of them mean the change was taken.
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

/** A ward's ACK that means it did NOT take the change (see the ACK table in
 *  spec/contract.md). Never counted as reached or applied. */
export type ProblemAck = "refused" | "failed" | "expired" | "superseded";

export function isProblemAck(state: LocalDeliveryState | undefined): state is ProblemAck {
  return state === "refused" || state === "failed" || state === "expired" || state === "superseded";
}

/** One device a multi-device send did not get to: the ward's own problem ACK,
 *  or the publish error (the relays failed and the carrier did not hold it). */
export interface FailedDevice {
  /** The device's machine key. */
  machine: string;
  ack?: ProblemAck;
  /** The guardian-facing error message, when there was no ACK. */
  error?: string;
}

/** "{device} didn't accept this change." — the plain words for a clause's problem ACK. */
export function clauseProblemText(device: string, ack: ProblemAck): string {
  switch (ack) {
    case "refused":
      return `${device} didn't accept this change.`;
    case "failed":
      return `${device} couldn't carry this out. It may need another try.`;
    case "expired":
      return `${device} didn't act on this: it had already expired there.`;
    case "superseded":
      return `${device} already had a newer change, so it didn't use this one.`;
  }
}

/**
 * The sentence for one device a send did not get to. `partial` is for a send
 * that DID reach or is held for another device: there, "try again" would
 * re-sign the change for that device too, so the invitation is left off.
 */
export function failedDeviceText(
  device: string,
  f: Pick<FailedDevice, "ack" | "error">,
  opts: { partial?: boolean } = {},
): string {
  if (f.ack) return clauseProblemText(device, f.ack);
  const why = f.error ?? "it couldn't be reached.";
  return `Not sent to ${device}: ${opts.partial ? why.replace(/ — check your connection and try again\.?$/, ".") : why}`;
}

/**
 * The carrier is holding the change for a ward device — queued for one that is
 * not on the home network right now, or sent to one that has not confirmed yet
 * — and the relays failed. It is delivered as soon as the ward reads it.
 * Callers show this as a WARNING: not success, and not a failure that invites a
 * retry. An ask answered this way is recorded as answered (the store's status
 * guards then stop a second, conflicting signature).
 *
 * A multi-device send uses the same error for any PARTIAL outcome: at least one
 * device reached or held, and the rest named in `devices` (held) or `failed`.
 */
export class QueuedForHomeNetworkError extends Error {
  /** Names of the devices the carrier is holding the change for. */
  readonly devices: string[];
  /**
   * Machine keys of the devices in the same batch that DID get the change (a
   * multi-device send where only some were away). Empty when none did, or for
   * a single wrap. The caller names them; this layer only knows keys.
   */
  readonly reached: string[];
  /** Devices in the same batch the change did not get to. */
  readonly failed: FailedDevice[];
  /** Ids of the batch's wraps the carrier took, so their ACKs can be shown. */
  readonly wraps: string[];
  constructor(
    devices: string[],
    reached: string[] = [],
    failed: FailedDevice[] = [],
    wraps: string[] = [],
  ) {
    const fallback = devices.length === 0 && reached.length === 0 && failed.length === 0;
    const names = [...new Set(fallback ? ["their device"] : devices)];
    super(QueuedForHomeNetworkError.describe(names, failed.length));
    this.name = "QueuedForHomeNetworkError";
    this.devices = names;
    this.reached = [...new Set(reached)];
    this.failed = failed;
    this.wraps = [...new Set(wraps)];
  }

  private static describe(names: string[], failed: number): string {
    if (names.length === 0) {
      return failed > 0 ? "Not sent to every device." : "Not sent yet.";
    }
    if (names.length === 1) {
      return `Not sent yet — it'll reach ${names[0]} when it's on the home network.`;
    }
    const last = names[names.length - 1];
    return `Not sent yet — it'll reach ${names.slice(0, -1).join(", ")} and ${last} when they're on the home network.`;
  }

  /** One error naming every device across several queued wraps. */
  static merge(
    errors: QueuedForHomeNetworkError[],
    reached: string[] = [],
    failed: FailedDevice[] = [],
    wraps: string[] = [],
  ): QueuedForHomeNetworkError {
    return new QueuedForHomeNetworkError(
      errors.flatMap((e) => e.devices),
      [...errors.flatMap((e) => e.reached), ...reached],
      [...errors.flatMap((e) => e.failed), ...failed],
      [...errors.flatMap((e) => e.wraps), ...wraps],
    );
  }
}

export const NO_RELAY_MESSAGE = "Couldn't reach any relay — check your connection and try again.";

/** What the home network saw of a publish that resolved. */
export interface PublishOutcome {
  /** The carrier took the wrap (sent or held), so a delivery record exists. */
  routed: boolean;
  /** The last home-network state seen for it, when the carrier took it. A
   *  problem ACK (`isProblemAck`) means the ward did NOT take the change. */
  local?: LocalDeliveryState;
}

type RelayPublish = (relays: string[], event: NostrEvent) => Promise<void>;
export type RoutedPublish = (
  relays: string[],
  event: NostrEvent,
  meta?: PublishMeta,
) => Promise<PublishOutcome>;

/** The single `p`-tag recipient of a kind-1059 wrap, else null. */
export function singleRecipient(event: NostrEvent): string | null {
  if (event.kind !== 1059) return null;
  const p = event.tags.filter((t) => t[0] === "p");
  if (p.length !== 1) return null;
  return /^[0-9a-f]{64}$/.test(p[0][1] ?? "") ? p[0][1] : null;
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

function isReceipt(state: LocalDeliveryState | undefined): boolean {
  // received, or a ward ACK that settles the wrap. `rejected` is the carrier
  // refusing the wrap; queued/sent mean no receipt yet; `ignored` is a kind
  // the ward does not act on over the link, so only the relay copy counts.
  return (
    state !== undefined &&
    state !== "queued" &&
    state !== "sent" &&
    state !== "rejected" &&
    state !== "ignored"
  );
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
    if (!recipient || !localRouteAvailable()) {
      await relayPublish(relays, event);
      return { routed: false };
    }
    const deviceName = () => hooks.deviceName?.(recipient) ?? "their device";

    // Both routes start now. The relay promise is observed here, once, so a
    // rejection after the local route has already succeeded can never surface
    // as an unhandled rejection; the race below re-uses the same promise.
    const relay: { state: "pending" | "ok" | "failed"; error?: unknown } = { state: "pending" };
    const relayRun: Promise<void> =
      relays.length === 0
        ? // Nothing to publish to: if the carrier can't take it either, say so
          // plainly rather than blame relays that were never configured.
          Promise.reject(new Error(`No relay is set up to reach ${deviceName()}, so this couldn't be sent.`))
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
    const routed = lastLocal !== undefined;
    const outcome = (): PublishOutcome => (routed ? { routed, local: lastLocal } : { routed });
    if (sent === "sent" || sent === "queued") {
      try {
        hooks.onRouted?.({ wrapId: event.id, machine: recipient, state: sent, meta });
      } catch {
        // Tracking is bookkeeping; it must never fail a delivery.
      }
    }

    if (sent === "sent") {
      const deadline = Date.now() + LOCAL_RECEIPT_WINDOW_MS;
      for (;;) {
        const entry = localDelivery([event.id])?.[event.id];
        if (entry) lastLocal = entry.state;
        if (isReceipt(lastLocal)) return outcome(); // the ward has it; relays carry on unobserved
        if (lastLocal === "rejected" || lastLocal === "ignored") break;
        // A relay that already took it is as good as a receipt for the caller —
        // no reason to hold the guardian for the rest of the window.
        if (relay.state === "ok") return outcome();
        if (Date.now() >= deadline) break;
        // Wake early if the relay settles meanwhile; once it has settled,
        // only the clock is worth waiting for.
        await (relay.state === "pending"
          ? Promise.race([sleep(LOCAL_POLL_MS), relayDone])
          : sleep(LOCAL_POLL_MS));
      }
    }

    await relayDone;
    if (relay.state === "ok") return outcome();

    // Relays failed. One last look: the receipt may have landed while we waited.
    if (lastLocal === "sent" || lastLocal === "queued") {
      const entry = localDelivery([event.id])?.[event.id];
      if (entry) lastLocal = entry.state;
    }
    if (isReceipt(lastLocal)) return outcome();
    // Held: queued for a ward that isn't connected, or sent and not yet
    // confirmed. Either way the carrier keeps it until the ward's OK and sends
    // it again at the next READY, so it WILL arrive — never a hard failure.
    if (lastLocal === "queued" || lastLocal === "sent") {
      throw new QueuedForHomeNetworkError([deviceName()], [], [], [event.id]);
    }
    throw relay.error;
  };
}
