// What the screens know about the home-network route: which ward devices have
// a live session with the carrier right now, and — when there is no usable
// route — the plain sentence that says why.

import { localBridgeSupport, localInfo, type LocalRouteGap } from "./localLink";

export interface LocalRouteView {
  /** null = home-network delivery is usable now. */
  gap: LocalRouteGap | null;
  /** Machine keys of ward devices with a live session. */
  sessions: ReadonlySet<string>;
}

export const NO_LOCAL_ROUTE: LocalRouteView = { gap: "browser", sessions: new Set() };

/**
 * One reading of the route: AT MOST ONE `localInfo()` call, however many
 * screens or rows want the answer. The store takes this on its own poll and
 * shares the result; nothing else should call it on a timer.
 */
export function readLocalRoute(): LocalRouteView {
  if (!window.CharterCarrier) return NO_LOCAL_ROUTE;
  if (localBridgeSupport() !== "full") return { gap: "old-carrier", sessions: new Set() };
  const info = localInfo();
  // Sessions are shown whenever the carrier reports them, even if it also says
  // the listener isn't fully available: a working session is never "offline".
  return {
    gap: info?.available === true ? null : "not-running",
    sessions: new Set(info?.sessions.map((s) => s.machine) ?? []),
  };
}

/** The line to show where relay delivery is discussed, or null when the route works. */
export function localRouteGapLine(gap: LocalRouteGap | null): string | null {
  switch (gap) {
    case null:
      return null;
    case "browser":
      return "Home-network delivery needs the Kintrinsic Android app. Here, changes travel through relays.";
    case "old-carrier":
      return "Update the Kintrinsic app to deliver over your home network.";
    case "not-running":
      return "Home-network delivery isn't running right now (no home Wi-Fi?). Changes travel through relays until it is.";
  }
}

export function sameLocalRoute(a: LocalRouteView, b: LocalRouteView): boolean {
  if (a.gap !== b.gap || a.sessions.size !== b.sessions.size) return false;
  for (const m of a.sessions) if (!b.sessions.has(m)) return false;
  return true;
}
