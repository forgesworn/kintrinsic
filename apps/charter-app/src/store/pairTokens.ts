// The pairing tokens THIS guardian has minted — the proof-of-pairing the
// unclaimed-device recovery checks against (S2, review 2026-08-07).
//
// A one-time token is 128 bits of CSPRNG that leaves this app in exactly two
// places: the QR a parent holds up, and the cable link fired at a phone they
// are holding. A phone that echoes one on its STATUS heartbeat is therefore a
// phone that was handed a pairing link BY this guardian — which is precisely
// what the claim flow needs to know and, before this file existed, only
// assumed.
//
// It has to be DURABLE, unlike the in-session `phonePairing` token: the whole
// point of the recovery is that the app lost track of the pairing. A ledger
// that dies with the tab would be gone in exactly the case it is needed —
// closed the app, reopened it, phone still unrecorded.
//
// It is NOT secret-bearing in the way the guardian key is. A token proves
// "this device was handed a link by this guardian" and nothing else: it signs
// nothing, decrypts nothing, and is worthless once its device is claimed. It
// lives beside the rest of the app's local state accordingly.

const KEY = "charter.pair.tokens.v1";

/**
 * How long a minted token stays claimable. Generous on purpose — the device's
 * own echo window is the real limit (the ward stamps `pairToken` on STATUS for
 * a bounded span after pairing and then never again), and this side should
 * never be the half that expires first and turns a recoverable split-brain
 * into a dead end.
 */
export const PAIR_TOKEN_MEMORY_MS = 24 * 60 * 60_000;

/** Most tokens kept. A guardian setting up a houseful of phones in one day
 *  will not pass this; a bug that mints in a loop cannot grow unbounded. */
const MAX_TOKENS = 32;

export interface MintedPairToken {
  token: string;
  /** Who the parent said they were pairing FOR when they showed the code. */
  childId: string;
  mintedAt: number;
}

function read(): MintedPairToken[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((t): t is MintedPairToken => {
      if (typeof t !== "object" || t === null) return false;
      const m = t as Record<string, unknown>;
      return (
        typeof m.token === "string" &&
        m.token.length > 0 &&
        typeof m.childId === "string" &&
        typeof m.mintedAt === "number" &&
        Number.isFinite(m.mintedAt)
      );
    });
  } catch {
    // A corrupt or unavailable store must fail CLOSED here: an empty ledger
    // offers no device for adoption, which is the safe direction. The parent's
    // way forward is to show the pairing code again, which mints afresh.
    return [];
  }
}

function write(tokens: MintedPairToken[]): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(tokens));
  } catch {
    // Out of quota / private mode: the claim path simply won't offer the
    // device. Never throw — this is called from the middle of showing a QR.
  }
}

/** Everything minted and still inside its window, newest first. */
export function mintedPairTokens(now: number = Date.now()): MintedPairToken[] {
  return read()
    .filter((t) => now - t.mintedAt <= PAIR_TOKEN_MEMORY_MS && t.mintedAt <= now)
    .sort((a, b) => b.mintedAt - a.mintedAt);
}

/** The live token set, for the claim gate. */
export function livePairTokens(now: number = Date.now()): Set<string> {
  return new Set(mintedPairTokens(now).map((t) => t.token));
}

/** Record a token we just put on screen (or down a cable). Prunes as it goes,
 *  so the ledger is trimmed by ordinary use rather than needing a sweeper. */
export function rememberPairToken(
  token: string,
  childId: string,
  now: number = Date.now(),
): void {
  const kept = mintedPairTokens(now).filter((t) => t.token !== token);
  write([{ token, childId, mintedAt: now }, ...kept].slice(0, MAX_TOKENS));
}

/** Drop a token once its device is recorded — a spent token should not sit
 *  around able to vouch for a second machine. */
export function forgetPairToken(token: string, now: number = Date.now()): void {
  write(mintedPairTokens(now).filter((t) => t.token !== token));
}
