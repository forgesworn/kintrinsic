// Relays Kintrinsic publishes gift-wrapped CLAUSEs to. The device subscribes on
// the relay it pinned from the guardian's `bunker://` at pairing, so both sides
// share the guardian's relay (Signet's production relay by default). MVP
// simplification: a single shared default; a fuller build derives the publish
// targets from each device's pairing.
export const DEFAULT_RELAYS = ["wss://relay.trotters.cc"];
