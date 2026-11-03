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

/** How often a read ignores the cursor and sweeps the whole floor window again.
 *
 *  A ward builds the gift-wrap for an emergency unlock at its first poll after
 *  the press, even offline, so the wrap's `created_at` is when it was built.
 *  If the phone was offline, the wrap reaches a relay long after that, and a
 *  running app whose cursor has already moved past that `created_at` would never
 *  ask for it again: only a cold start would find it. A periodic sweep closes
 *  that gap. Wraps already handled cost no decryption, so a sweep costs only the
 *  download. */
export const SWEEP_INTERVAL_SECS = 15 * 60;

/**
 * Should this read be a sweep of the whole floor window rather than the narrow
 * cursor window?
 *
 * Yes when none has happened yet this session, when the last one is at least
 * [`SWEEP_INTERVAL_SECS`] old, or when `forced` (the app has just come back to
 * the foreground, or the guardian pulled to refresh: someone is looking now).
 * `lastSweepAt` is the start time of the last sweep that SUCCEEDED; a failed
 * read covered nothing, so the caller does not record it.
 */
export function sweepDue(lastSweepAt: number | null, nowSecs: number, forced = false): boolean {
  if (forced || lastSweepAt === null) return true;
  return nowSecs - lastSweepAt >= SWEEP_INTERVAL_SECS;
}

/**
 * The `since` for the next read, and whether it is a sweep. A sweep reads from
 * the floor regardless of the cursor; any other read follows [`windowSince`].
 */
export function readWindow(
  cursor: number | null,
  lastSweepAt: number | null,
  nowSecs: number,
  forced = false,
): { since: number; sweep: boolean } {
  const sweep = sweepDue(lastSweepAt, nowSecs, forced);
  return {
    since: sweep ? nowSecs - WINDOW_FLOOR_SECS : windowSince(cursor, nowSecs),
    sweep,
  };
}

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
