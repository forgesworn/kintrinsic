import type { Child } from "../domain/types";
import { resolveRoute } from "../routes";

// Where the bottom bar shows, and which of its buttons is current.

/** Which button is current: Home (only with no children), one child, Settings,
 *  or none (Activity). */
export type BottomBarActive = "home" | "settings" | { childId: string } | null;

/**
 * Whether the bar shows on a route (the hash without `#/`), and which button
 * is current. Hidden on the full-screen pages that have their own pinned
 * actions (rule editors, device pages, add-device) and on set-up.
 */
export function bottomBarFor(
  route: string,
  children: Child[],
  waiting?: ReadonlySet<string>,
): { show: boolean; active: BottomBarActive } {
  const hidden = { show: false, active: null } as const;
  const r = resolveRoute(route, children, waiting);
  switch (r.kind) {
    case "setup":
    case "rule":
    case "device":
    case "add-device":
      return hidden;
    case "home":
      return { show: true, active: "home" };
    case "settings":
      return { show: true, active: "settings" };
    case "activity":
      return { show: true, active: null };
    case "ward-settings":
      return { show: true, active: { childId: r.childId } };
    case "ward": {
      // `ward/<id>/rule/app/<policyId>` (the page for one game or app) is read
      // ahead of the route table, which sees it as the child page. (A route
      // that was only tidied onto a child's page, `canonical`, is not that.)
      const parts = route.split("/").filter((p) => p !== "");
      if (parts[2] === "rule" && !r.canonical) return hidden;
      return { show: true, active: { childId: r.childId } };
    }
    default:
      return { show: true, active: null };
  }
}
