import { finalizeEvent, verifyEvent, type EventTemplate, type NostrEvent, type VerifiedEvent } from "nostr-tools";

/** Sign only the connection proof the relay requested. It cannot authorise a
 * charter change or expose the guardian secret to the relay. */
export function relayAuthSigner(signEvent: (template: EventTemplate) => Promise<NostrEvent>) {
  return async (template: EventTemplate): Promise<VerifiedEvent> => {
    if (template.kind !== 22242 || template.content !== "" ||
      template.tags.length !== 2 ||
      !template.tags.some((t) => t.length === 2 && t[0] === "relay" && t[1].startsWith("wss://")) ||
      !template.tags.some((t) => t.length === 2 && t[0] === "challenge" && t[1].length > 0 && t[1].length <= 4096)) {
      throw new Error("Invalid relay authentication request.");
    }
    const event = await signEvent(template);
    if (!verifyEvent(event)) throw new Error("Relay authentication signature is invalid.");
    return event;
  };
}

export function localRelayAuth(secret: Uint8Array) {
  return relayAuthSigner(async (template) => finalizeEvent(template, secret));
}
