import { useEffect, useRef, useState } from "react";
import { Avatar, Badge } from "./ui";
import { wardPath } from "../domain/wardRoute";
import { bottomBarFor } from "./bottomBarFor";
import { waitingChildIds } from "../routes";
import { useCharter } from "../store/store";
import { useLocalRoute } from "../carrier/useLocalRoute";
import { childGlance } from "./childGlance";

// The bottom bar: one button per child, then Settings. The bottom is where a
// thumb reaches, so this is the main way between the app's top-level pages.
//
// Settings is pinned at the right; the children fill the rest in the family's
// order (Settings -> "Children and family devices") and the strip scrolls
// sideways once they no longer fit. There is no Home while there are children
// (the app opens on the first); with none, Home, the getting-started page, is
// the first button.
//
// Each child's button is a small pill with a glance at their state: whether
// they are online, and the time left today (or locked / time's up). The
// glance is worked out with the same code as the child page's "Now" card, and
// redrawn every 30 seconds so "left" does not go stale on a screen left open.

const GLANCE_REFRESH_MS = 30_000;

export function BottomBar({ route }: { route: string }) {
  const { state, deviceStatus } = useCharter();
  const { sessions } = useLocalRoute();
  const { show, active } = bottomBarFor(route, state.children, waitingChildIds(state.requests));
  const stripRef = useRef<HTMLDivElement>(null);
  const [more, setMore] = useState({ left: false, right: false });
  const activeChild = typeof active === "object" && active ? active.childId : null;
  const [, setTick] = useState(0);

  // Keep the glance current between store updates: the render below reads the
  // clock, so a tick is all it needs.
  useEffect(() => {
    const id = setInterval(() => setTick((t) => t + 1), GLANCE_REFRESH_MS);
    return () => clearInterval(id);
  }, []);

  // Which sides have more children beyond the edge, for the fades.
  useEffect(() => {
    const el = stripRef.current;
    if (!el) return;
    const measure = () =>
      setMore((prev) => {
        const left = el.scrollLeft > 1;
        const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
        return prev.left === left && prev.right === right ? prev : { left, right };
      });
    measure();
    el.addEventListener("scroll", measure, { passive: true });
    window.addEventListener("resize", measure);
    return () => {
      el.removeEventListener("scroll", measure);
      window.removeEventListener("resize", measure);
    };
  }, [show, state.children.length]);

  // Bring the current child into view when the route changes. Scrolls the
  // strip itself (not the page) and only as far as needed.
  useEffect(() => {
    const strip = stripRef.current;
    if (!strip || activeChild === null) return;
    const el = strip.querySelector<HTMLElement>('[aria-current="page"]');
    if (!el) return;
    const left = el.offsetLeft;
    const right = left + el.offsetWidth;
    if (left < strip.scrollLeft) strip.scrollTo?.({ left: left - 8, behavior: "smooth" });
    else if (right > strip.scrollLeft + strip.clientWidth)
      strip.scrollTo?.({ left: right - strip.clientWidth + 8, behavior: "smooth" });
  }, [activeChild, show]);

  if (!show) return null;

  const pending = new Map<string, number>();
  for (const r of state.requests) {
    if (r.status === "pending") pending.set(r.childId, (pending.get(r.childId) ?? 0) + 1);
  }
  const go = (to: string) => {
    window.location.hash = `/${to}`;
  };

  return (
    <nav className="tabbar" aria-label="Main">
      {state.children.length === 0 && (
        <button
          type="button"
          className="tab"
          aria-current={active === "home" ? "page" : undefined}
          onClick={() => go("home")}
        >
          <span className="tab-icon" aria-hidden="true">
            🏠
          </span>
          <span className="tab-label">Home</span>
        </button>
      )}

      <div
        className="tabbar-strip"
        data-more-left={more.left || undefined}
        data-more-right={more.right || undefined}
      >
        <div className="tabbar-children" ref={stripRef}>
          {state.children.map((child) => {
            const count = pending.get(child.id) ?? 0;
            const current = activeChild === child.id;
            const glance = childGlance(child, deviceStatus, Date.now(), sessions);
            return (
              <button
                key={child.id}
                type="button"
                className="tab tab-child"
                aria-current={current ? "page" : undefined}
                aria-label={count > 0 ? `${glance.spoken}, ${count} waiting` : glance.spoken}
                onClick={() => go(wardPath(child.id))}
              >
                <span className="tab-avatar" aria-hidden="true">
                  <Avatar name={child.name} color={child.color} size={30} />
                  <span className="tab-dot" data-online={glance.online || undefined} />
                  {count > 0 && (
                    <span className="tab-badge">
                      <Badge count={count} />
                    </span>
                  )}
                </span>
                <span className="tab-text" aria-hidden="true">
                  <span className="tab-label">{child.name}</span>
                  <span className="tab-glance" data-tone={glance.tone}>
                    {glance.line}
                  </span>
                  {glance.fraction !== null && (
                    <span className="tab-meter">
                      <span
                        className="tab-meter-fill"
                        style={{ width: `${glance.fraction * 100}%`, background: child.color }}
                      />
                    </span>
                  )}
                </span>
              </button>
            );
          })}
        </div>
      </div>

      <button
        type="button"
        className="tab"
        aria-current={active === "settings" ? "page" : undefined}
        onClick={() => go("settings")}
      >
        <span className="tab-icon" aria-hidden="true">
          ⚙
        </span>
        <span className="tab-label">Settings</span>
      </button>
    </nav>
  );
}
