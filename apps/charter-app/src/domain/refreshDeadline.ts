// A manual refresh must always END — visibly. The relay read has no deadline
// of its own (querySync owns its sockets and, with no connectivity, can dangle
// far past anyone's patience: decented sat watching "Checking…" in airplane mode,
// 2026-08-10). The deadline governs the UI, not the work: the poll is not
// cancellable and a late success should still land its data — the guardian
// just isn't left staring at a spinner that will never admit defeat.

export const REFRESH_DEADLINE_MS = 10_000;

export type RefreshOutcome = "done" | "failed" | "timeout";

/** Resolves when the work settles or the deadline passes — never rejects. */
export function raceRefresh(
  work: Promise<unknown>,
  deadlineMs: number = REFRESH_DEADLINE_MS,
): Promise<RefreshOutcome> {
  return Promise.race([
    work.then(
      () => "done" as const,
      () => "failed" as const,
    ),
    new Promise<"timeout">((resolve) => {
      setTimeout(() => resolve("timeout"), deadlineMs);
    }),
  ]);
}

/** One honest line for both causes — the app cannot tell a downed relay from
 *  airplane mode, so it should not pretend to. */
export const REFRESH_OFFLINE_NOTICE =
  "Couldn't check — offline, or the relays aren't answering";
