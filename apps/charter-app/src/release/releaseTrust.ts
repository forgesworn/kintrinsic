// The D2 release trust anchors: which key announces software releases, on
// which relays, for which channels. The pubkey is a COMPILED-IN pin — the
// matching secret lives outside every repo (see android/keystore/README.md,
// "The Nostr release key"). Rotating it means shipping new clients.

/** Addressable software-release announcements (Zapstore-aligned kind). */
export const SOFTWARE_RELEASE_KIND = 30063;

/** x-only pubkey of the release key (ceremony 2026-08-12). */
export const RELEASE_PUBKEY_HEX =
  "11ecfbc95f61796b0b0c5156a24edb324b0ea5e69ff659990b99ebe2f44043a0";

/**
 * Where release events are published and looked for. Deliberately wider than
 * DEFAULT_RELAYS: trotters must never be load-bearing (the end state runs no
 * services of ours), so releases always ride public relays too.
 */
export const RELEASE_RELAYS = [
  "wss://relay.trotters.cc",
  "wss://relay.damus.io",
  "wss://nos.lol",
];

/** One d-tag per artifact. Exact strings — the publisher writes them too. */
export type ReleaseChannel = "charter-apk" | "mycharter-apk" | "charter-deb";
export const RELEASE_CHANNELS: readonly ReleaseChannel[] = [
  "charter-apk",
  "mycharter-apk",
  "charter-deb",
];
