import { nip19 } from "nostr-tools";

/**
 * The guardian identity the laptop pins at setup:
 *   `bunker://<guardian-pubkey>?relay=wss://…&kind=charter`
 * (`spec/contract.md`, "Pairing — bunker:// grammar"). The laptop only PINS this
 * pubkey + relay; it never dials a bunker — so a local-key PWA advertises its own
 * pubkey here exactly as a Signet bunker would.
 */
export function guardianBunkerUri(pubkeyHex: string, relays: string[]): string {
  // Relay values ride UNENCODED: the contract grammar requires a literal
  // `relay=wss://…` and the device-side parser pins on that prefix. (It now
  // also percent-decodes defensively — repairing links minted by older builds
  // of this app, which encoded here and produced never-pinnable URIs.) Encode
  // only characters that would break query parsing; plain wss URLs have none.
  const relayParams = relays
    .map((r) => `relay=${r.split("&").join("%26").split("#").join("%23")}`)
    .join("&");
  return `bunker://${pubkeyHex}?${relayParams}&kind=charter`;
}

/** The same guardian key as an `npub…`, for human-readable display. */
export function guardianNpub(pubkeyHex: string): string {
  return nip19.npubEncode(pubkeyHex);
}

/**
 * The QR-scannable pairing link: the https App Link form whose #fragment
 * carries the bunker URI plus a one-time `token`. Scanned by the CHILD phone's
 * system camera, it opens the Kintrinsic app directly (verified App Link; the
 * fragment never reaches any server). The phone echoes the token on its STATUS
 * heartbeat, which is how Kintrinsic recognises the device that scanned THIS
 * code and binds its machine pubkey — no typing in either direction.
 */
export function guardianPairingQrContent(
  pubkeyHex: string,
  relays: string[],
  token: string,
): string {
  return `https://charter.mysignet.app/pair#${guardianBunkerUri(pubkeyHex, relays)}&token=${token}`;
}

/**
 * The raw bunker:// pairing URI WITH a one-time token — fired at the phone
 * over the cable (am start), same token the QR flow embeds. MainActivity's
 * `bunker://` scheme handler is unambiguous on-device; the https App Link
 * form (`guardianPairingQrContent`) risks opening a browser instead when
 * launched via an adb intent, so the cable flow uses this raw form.
 */
export function guardianPairingBunkerUri(
  pubkeyHex: string,
  relays: string[],
  token: string,
): string {
  return `${guardianBunkerUri(pubkeyHex, relays)}&token=${token}`;
}

/** A fresh one-time pairing token (32 hex chars from the CSPRNG). */
export function mintPairToken(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}
