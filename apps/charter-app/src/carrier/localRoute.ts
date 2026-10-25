// What the screens know about the home-network route: which ward devices have
// a live session with the carrier right now, and — when there is no usable
// route — the plain sentence that says why.

import { useEffect, useState } from "react";
import { localInfo, localRouteGap, type LocalRouteGap } from "./localLink";

export interface LocalRouteView {
  /** null = home-network delivery is usable now. */
  gap: LocalRouteGap | null;
  /** Machine keys of ward devices with a live session. */
  sessions: ReadonlySet<string>;
}

export const NO_LOCAL_ROUTE: LocalRouteView = { gap: "browser", sessions: new Set() };

export function readLocalRoute(): LocalRouteView {
  const gap = localRouteGap();
  // Sessions are shown whenever the carrier reports them, even if it also says
  // the listener isn't fully available: a working session is never "offline".
  const info = gap === "browser" || gap === "old-carrier" ? null : localInfo();
  return { gap, sessions: new Set(info?.sessions.map((s) => s.machine) ?? []) };
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

function same(a: LocalRouteView, b: LocalRouteView): boolean {
  if (a.gap !== b.gap || a.sessions.size !== b.sessions.size) return false;
  for (const m of a.sessions) if (!b.sessions.has(m)) return false;
  return true;
}

/** Live view of the route, refreshed while the app is on screen. */
export function useLocalRoute(pollMs = 4_000): LocalRouteView {
  const [view, setView] = useState<LocalRouteView>(() => readLocalRoute());
  useEffect(() => {
    const refresh = () => {
      if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
      const next = readLocalRoute();
      setView((cur) => (same(cur, next) ? cur : next));
    };
    refresh();
    // A plain browser has no bridge to poll; the gap line never changes there.
    if (typeof window.CharterCarrier === "undefined") return;
    const id = setInterval(refresh, pollMs);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [pollMs]);
  return view;
}
