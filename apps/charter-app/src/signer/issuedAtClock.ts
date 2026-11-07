/**
 * The install's one rising clock for every time it signs, or asks its root to
 * sign: clauses, statements, the guardian PIN's `created_at`, presence
 * confirmations and rosters.
 *
 * A ward keeps a replay floor per seal author and slot (contract, *Who may
 * sign what*), and refuses an event at or below it. That rests on `issuedAt`
 * only ever rising per install. A device clock that was once ahead, and was
 * then corrected, would otherwise date the next event below the floor its
 * earlier one set, and that slot would be shut to this install until real
 * time caught up. So each time handed out is now, but strictly above every one
 * handed out before, and it is persisted so a reload can't step back under it.
 *
 * One counter per install: it lives in `localStorage` and is read afresh on
 * every call, so the clause signer and the guardian-keys module, which each
 * hold their own instance, share it. Each instance also remembers what it
 * handed out, for when storage can't be read or written (private mode). The key is the clause signer's old one, so an
 * install keeps the floor it already has.
 *
 * Not for a relay-list RELAYS: a ward refuses one dated ahead of its clock,
 * and its floor is the ward's clock, so that is dated with the raw clock.
 */
export const ISSUED_AT_KEY = "charter.signer.lastIssuedAt";

export class IssuedAtClock {
  /** The last time handed out, for when storage can't be read. */
  private memory = 0;

  /** The last time handed out (0 for none). */
  last(): number {
    try {
      const n = Number(localStorage.getItem(ISSUED_AT_KEY));
      return Math.max(Number.isFinite(n) && n > 0 ? n : 0, this.memory);
    } catch {
      return this.memory;
    }
  }

  /**
   * The next time to sign with: `candidate` (normally now), raised to one
   * second above the last time handed out, and recorded.
   */
  next(candidate: number): number {
    const at = Math.max(candidate, this.last() + 1);
    this.memory = at;
    try {
      localStorage.setItem(ISSUED_AT_KEY, String(at));
    } catch {
      // Private mode / quota: the in-memory floor still holds this session.
    }
    return at;
  }
}
