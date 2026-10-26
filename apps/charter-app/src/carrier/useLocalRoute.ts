// The hook screens use to read the home-network route. Kept apart from
// `localRoute.ts` so the store (which owns the poll and imports that file) and
// the screens (which import the store) never form an import cycle.

import { useCharter } from "../store/store";
import { NO_LOCAL_ROUTE, type LocalRouteView } from "./localRoute";

/**
 * The route as the store last read it (its 3 s poll, and on returning to the
 * app). Reads the shared value — it never polls the bridge itself, so ten
 * device rows cost the same single `localInfo()` call as one.
 */
export function useLocalRoute(): LocalRouteView {
  return useCharter().localRoute ?? NO_LOCAL_ROUTE;
}
