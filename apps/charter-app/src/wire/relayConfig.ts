import type { EventTemplate } from "nostr-tools";

export const CHARTER_DEVICE_RELAYS = 31118;

/** Keep in step with charter-transport::relay_config::validate_relays. */
export function validateRelays(relays: string[]): string[] {
  if (relays.length > 10) throw new Error("Choose between two and ten secure relays.");
  const values = relays.map((relay) => {
    let url: URL;
    try { url = new URL(relay.trim()); }
    catch { throw new Error("Invalid relay URL."); }
    if (url.protocol !== "wss:" || !url.hostname || url.username || url.password || url.href.includes("#")) {
      throw new Error("Each relay must be a wss:// URL without credentials or a fragment.");
    }
    return url.pathname === "/" && !url.href.includes("?") ? url.href.slice(0, -1) : url.href;
  });
  const unique = [...new Set(values)];
  if (unique.length < 2) throw new Error("Choose at least two different secure relays.");
  return unique;
}

export function relayConfigTemplate(machine: string, relays: string[], now: number): EventTemplate {
  if (!/^[0-9a-f]{64}$/.test(machine)) throw new Error("This device has no valid device code.");
  return {
    kind: CHARTER_DEVICE_RELAYS,
    created_at: now,
    tags: [["t", "charter-device"], ["p", machine]],
    content: JSON.stringify({ machine, relays: validateRelays(relays) }),
  };
}
