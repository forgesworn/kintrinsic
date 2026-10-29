import type { Child, ChildRequest } from "./domain/types";
import { wardPath } from "./domain/wardRoute";
import { SECTION_TITLE, type SectionId } from "./screens/Limits";

// The guardian app's hash routes, resolved in one place.
//
// Every page is a hash route, so Android's back button and the carrier's
// notification deep links (`#/approvals`, `#/activity`) work with no native
// change. The old tab routes keep landing somewhere sensible: `approvals`
// and `family` are no longer screens of their own, they redirect into a
// child's page and Settings.
//
// Once the family has a child there is no Home: the empty route, `home`, an
// unknown route and a child who no longer exists all land on the first child
// (in the order set in Settings). Home is only the getting-started page for a
// family with no children yet.
//
// A pure function of the route string and the family, so the whole table is
// testable without mounting anything.

export type ResolvedRoute =
  | { kind: "setup" }
  | { kind: "home" }
  | { kind: "ward"; childId: string; scroll?: "waiting"; canonical?: string }
  | { kind: "ward-settings"; childId: string }
  | { kind: "rule"; childId: string; section: SectionId }
  | { kind: "device"; childId: string; deviceId: string }
  | { kind: "add-device"; childId: string }
  | { kind: "settings"; sub: string }
  | { kind: "activity" }
  | { kind: "limits" };

const HOME: ResolvedRoute = { kind: "home" };

const SECTION_IDS = Object.keys(SECTION_TITLE) as SectionId[];

function seg(s: string | undefined): string {
  if (!s) return "";
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** The children with an ask waiting, for the bare `approvals` link. */
export function waitingChildIds(requests: ChildRequest[]): ReadonlySet<string> {
  const ids = new Set<string>();
  for (const r of requests) if (r.status === "pending") ids.add(r.childId);
  return ids;
}

/**
 * Resolve a route (the hash without `#/`) against the family.
 *
 * An unknown route, or one naming a child who no longer exists, lands on the
 * first child (Home when there are none) rather than on a blank page: removing
 * a child must not strand a stale link. `canonical`, when set, is the hash the
 * address should be replaced with (an `approvals/<id>` link becomes
 * `ward/<id>`). `waiting` names the children with an ask waiting: the bare
 * `approvals` link opens the first of them (in `children` order).
 */
export function resolveRoute(
  route: string,
  children: Child[],
  waiting: ReadonlySet<string> = new Set(),
): ResolvedRoute {
  const parts = route.split("/").filter((p) => p !== "");
  const head = parts[0] ?? "";
  const known = (id: string): Child | undefined => children.find((c) => c.id === id);
  // Where "no such page" lands: the first child, or Home with none.
  const first = children[0];
  const fallback: ResolvedRoute = first
    ? { kind: "ward", childId: first.id, canonical: wardPath(first.id) }
    : HOME;

  switch (head) {
    case "":
    case "home":
      return fallback;
    case "setup":
      return { kind: "setup" };
    case "settings":
      return { kind: "settings", sub: parts.slice(1).map(seg).join("/") };
    case "family":
      // The old Family tab is Settings.
      return { kind: "settings", sub: "" };
    case "activity":
      return { kind: "activity" };
    case "limits":
      return { kind: "limits" };
    case "approvals": {
      const id = seg(parts[1]);
      if (!id) {
        const target = children.find((c) => waiting.has(c.id)) ?? first;
        return target
          ? { kind: "ward", childId: target.id, scroll: "waiting", canonical: wardPath(target.id) }
          : HOME;
      }
      return known(id)
        ? { kind: "ward", childId: id, scroll: "waiting", canonical: wardPath(id) }
        : fallback;
    }
    case "ward": {
      const id = seg(parts[1]);
      const child = known(id);
      if (!child) return fallback;
      const page: ResolvedRoute = { kind: "ward", childId: id };
      switch (parts[2]) {
        case undefined:
          return page;
        case "settings":
          return { kind: "ward-settings", childId: id };
        case "rule": {
          const section = seg(parts[3]);
          return SECTION_IDS.includes(section as SectionId)
            ? { kind: "rule", childId: id, section: section as SectionId }
            : page;
        }
        case "device": {
          const deviceId = seg(parts[3]);
          return child.devices.some((d) => d.id === deviceId)
            ? { kind: "device", childId: id, deviceId }
            : page;
        }
        case "add-device":
          return { kind: "add-device", childId: id };
        default:
          return page;
      }
    }
    default:
      return fallback;
  }
}
