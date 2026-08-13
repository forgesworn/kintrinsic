import { useEffect, useState } from "react";

// Routes that carry a subject: `#/limits/<childId>`, `#/activity/<childId>`.
//
// Both screens need the same three things — read the ward out of the address,
// keep the address in step when the switcher moves, and follow the browser's
// back button. They had one copy each of the first (Limits) and none of the
// rest (Activity's filter was invisible to the URL, so nothing could link to a
// particular ward's week). One hook, used by both.

/** The child id in `#/<tab>/<childId>`, or "" when the route carries none. */
export function wardIdFromHash(tab: string, hash = window.location.hash): string {
  const parts = hash.replace(/^#\/?/, "").split("/");
  return parts[0] === tab && parts[1] ? decodeURIComponent(parts[1]) : "";
}

/** Navigate to a tab, focused on one ward. */
export function goToWard(tab: string, childId: string): void {
  window.location.hash = `/${tab}/${encodeURIComponent(childId)}`;
}

/**
 * The ward this screen is showing, and a setter that keeps the address in step.
 *
 * `fallback` is what to select when the route names nobody — the first ward for
 * Limits, the "everyone" sentinel for Activity. A route id that no longer names
 * a real ward is ignored rather than selected, so removing a child can't leave a
 * screen pointed at a ghost.
 */
export function useWardRoute(
  tab: string,
  isKnown: (id: string) => boolean,
  fallback: string,
): [string, (id: string) => void] {
  const [selected, setSelected] = useState<string>(() => {
    const fromHash = wardIdFromHash(tab);
    return fromHash && isKnown(fromHash) ? fromHash : fallback;
  });

  useEffect(() => {
    const onHash = () => {
      const id = wardIdFromHash(tab);
      if (id) setSelected(id);
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, [tab]);

  const select = (id: string) => {
    setSelected(id);
    // Keep the address in step, so back returns to the ward you came from and
    // the choice survives a refresh.
    window.location.hash = id ? `/${tab}/${encodeURIComponent(id)}` : `/${tab}`;
  };

  return [selected, select];
}
