// Reading the persisted household back, without trusting it.
//
// `loadState` used to be `JSON.parse(raw) as CharterState`. A blob that would
// not parse (a write cut short by quota or a killed tab) fell through to an
// EMPTY household, and the persist-on-every-change effect then wrote that
// empty state over the damaged-but-partly-recoverable blob — the family gone,
// silently. A blob that parsed into the wrong shape (`{}`) reached the reducer
// and white-screened on the first `state.children.map`. The guardian KEY has
// had an absent-vs-corrupt distinction for a long time; this is the same
// distinction for the family it guards.

export interface RevivableState {
  children: unknown[];
  requests: unknown[];
  activity: unknown[];
  signer: { connected: boolean; kind: string; autoSign: boolean };
}

export type Revived<S> =
  | { kind: "absent" }
  | { kind: "ok"; state: S }
  /** Present but unusable — the caller must set `raw` aside before anything
   *  is persisted over it. */
  | { kind: "broken"; raw: string };

/**
 * `children` is the household: without an array there, nothing is usable. The
 * other fields are repairable — a missing or mis-shaped one takes `empty`'s
 * value rather than costing the family their children.
 */
export function reviveState<S extends RevivableState>(raw: string | null, empty: S): Revived<S> {
  if (raw === null || raw === "") return { kind: "absent" };
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { kind: "broken", raw };
  }
  if (typeof parsed !== "object" || parsed === null) return { kind: "broken", raw };
  const p = parsed as Partial<RevivableState>;
  if (!Array.isArray(p.children)) return { kind: "broken", raw };
  const signer =
    typeof p.signer === "object" && p.signer !== null ? { ...empty.signer, ...p.signer } : empty.signer;
  return {
    kind: "ok",
    state: {
      ...empty,
      ...(parsed as object),
      children: p.children,
      requests: Array.isArray(p.requests) ? p.requests : empty.requests,
      activity: Array.isArray(p.activity) ? p.activity : empty.activity,
      signer,
    } as S,
  };
}
