// Relays Kintrinsic publishes gift-wrapped CLAUSEs to for a NEW pairing. The
// device subscribes on the relay it pinned from the guardian's `bunker://` at
// pairing time, and that relay list is then stored on the pairing itself
// (`Device.relays`) — a device already paired never re-reads this constant.
// Kintrinsic is decentralised by design: no relay run by the project is ever
// a default — these are public relays, and a family that wants its own relay
// adds it themselves.
export const DEFAULT_RELAYS = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"];

/**
 * The ONE relay every pairing used before `Device.relays` existed. Used
 * ONLY to back-fill a legacy pairing's stored record the first time it is
 * loaded after this change (`store/pairingMigration.ts`) — it is NOT a
 * default and must never be read by anything that pairs a NEW device.
 */
export const LEGACY_PAIRING_RELAYS = ["wss://relay.trotters.cc"];
