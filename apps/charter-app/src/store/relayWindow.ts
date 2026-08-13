/**
 * How wide the next relay read reaches, and how far the cursor may move.
 *
 * The guardian's app used to ask every relay for a full day of gift-wraps every
 * thirty seconds and decrypt the lot before anything could recognise it as old
 * news. The window here is what replaced that. It is small, but it carries the
 * one property the wide window existed to give — an ask that could not be
 * ingested yet gets another chance — so it is worth stating on its own, away
 * from the store's plumbing, where it can be tested.
 */

/** The widest the window ever reaches back. Matches how long relays keep a
 *  wrap, and how long an unanswered ask stays worth showing a guardian. */
export const WINDOW_FLOOR_SECS = 24 * 60 * 60;

/** How far the cursor rewinds each round, to absorb the clock difference
 *  between the phone that published a wrap and the one reading it. */
export const CURSOR_OVERLAP_SECS = 120;

/**
 * The `since` for the next read: the cursor if we have one, but never reaching
 * further back than the floor.
 *
 * A session's first read has no cursor and sweeps the whole day, which is what
 * makes a reload heal anything the running app got wrong.
 */
export function windowSince(cursor: number | null, nowSecs: number): number {
  return Math.max(cursor ?? 0, nowSecs - WINDOW_FLOOR_SECS);
}

/**
 * Where the cursor stands after a read.
 *
 * `unfinished` is the whole point: a wrap this round could not complete (an ask
 * from a phone the guardian has not added yet) leaves the cursor exactly where
 * it was, so the next read covers that wrap again instead of stepping over it.
 * The floor in [`windowSince`] keeps that from pinning the window open forever
 * — a wrap nothing can ever ingest simply ages out after a day, which is the
 * behaviour the old always-24h read had anyway.
 *
 * A failed read is not a completed one: callers keep their cursor rather than
 * calling this, because a read that threw has covered nothing.
 */
export function nextCursor(
  cursor: number | null,
  startedAt: number,
  unfinished: boolean,
): number | null {
  if (unfinished) return cursor;
  return startedAt - CURSOR_OVERLAP_SECS;
}
