// Parse a device's pairing code (shown by charterd at setup) into its pubkey
// hex — the gift-wrap recipient. Accepts an `npub1…`, an `nprofile1…` (pubkey +
// relays), or a raw 64-hex key. Returns null for anything else, so the pairing
// UI can reject a typo before a clause is ever addressed to a bad key.

import { nip19 } from "nostr-tools";

/** A scanned scan-to-pair invite: the ward's machine key + its one-time token. */
export type PairInvite = { machine: string; token: string };

/**
 * Parse the scan-to-pair QR a ward shows on its Connect screen:
 * `charter://pair?m=<64hex>&t=<32hex>`.
 *
 * Returns null for anything else — including a bare device code, which is the
 * older/offline form and carries no token, so it cannot finish a pairing by
 * itself. Callers fall back to `parseDevicePairingCode` for that.
 */
export function parsePairInvite(code: string): PairInvite | null {
  const c = code.trim();
  if (!c.startsWith("charter://pair?")) return null;
  const q = new URLSearchParams(c.slice("charter://pair?".length));
  const machine = (q.get("m") ?? "").toLowerCase();
  const token = (q.get("t") ?? "").toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(machine)) return null;
  if (!/^[0-9a-f]{32}$/.test(token)) return null;
  return { machine, token };
}

export function parseDevicePairingCode(code: string): string | null {
  // A scan-to-pair URI carries the machine key too, so the existing "which
  // device is this?" callers keep working unchanged off the same scan.
  const invite = parsePairInvite(code);
  if (invite) return invite.machine;

  // charterd shows the code grouped into space-separated 8-char blocks for
  // readability (see `grouped_for_display`); a parent typing it back keeps those
  // spaces. Strip ALL whitespace — not just the ends — before matching the
  // 64-hex form, so the on-screen grouped code round-trips. (Bech32 entities
  // carry no internal spaces, so this is a no-op for npub/nprofile.)
  const c = code.replace(/\s+/g, "");
  if (/^[0-9a-fA-F]{64}$/.test(c)) return c.toLowerCase();
  try {
    const decoded = nip19.decode(c);
    if (decoded.type === "npub") return decoded.data;
    if (decoded.type === "nprofile") return decoded.data.pubkey;
  } catch {
    // not a recognised bech32 entity
  }
  return null;
}
