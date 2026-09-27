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
 * Where release events are published and looked for. No relay run by the
 * project is ever a default (Kintrinsic is decentralised by design) — these
 * are public relays, wider than DEFAULT_RELAYS on purpose so a release is
 * never load-bearing on a single one.
 */
export const RELEASE_RELAYS = [
  "wss://relay.damus.io",
  "wss://nos.lol",
  "wss://relay.primal.net",
];

/** One d-tag per artifact. Exact strings — the publisher writes them too. */
export type ReleaseChannel = "charter-apk" | "mycharter-apk" | "charter-deb";
export const RELEASE_CHANNELS: readonly ReleaseChannel[] = [
  "charter-apk",
  "mycharter-apk",
  "charter-deb",
];
