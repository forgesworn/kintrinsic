// Pull down at the top of the app to re-read everything (decented asked for it
// 2026-07-26, having had to close and reopen Kintrinsic to see a device's new
// version). The store's focus-refresh already covers coming back to the app;
// this is for when the guardian is sitting ON the screen, waiting for a device
// to report in, and wants to ask again NOW.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { useCharter } from "../store/store";
import { PULL_THRESHOLD, pullOffset, willRefresh } from "../domain/pullGesture";

export function PullToRefresh({ children }: { children: ReactNode }) {
  const { refreshNow, refreshing, refreshNotice } = useCharter();
  const [offset, setOffset] = useState(0);
  // The touch handlers are attached once (they must be non-passive to be
  // allowed to preventDefault), so they read live values through refs.
  const offsetRef = useRef(0);
  const startY = useRef<number | null>(null);
  const refresh = useRef(refreshNow);
  refresh.current = refreshNow;

  const setPull = (px: number) => {
    offsetRef.current = px;
    setOffset(px);
  };

  useEffect(() => {
    const onStart = (e: TouchEvent) => {
      // Only from a genuine top-of-page, single-finger pull: mid-scroll and
      // pinch gestures belong to the page, not to us.
      if (e.touches.length !== 1 || window.scrollY > 0) {
        startY.current = null;
        return;
      }
      startY.current = e.touches[0].clientY;
    };

    const onMove = (e: TouchEvent) => {
      if (startY.current === null) return;
      const dy = e.touches[0].clientY - startY.current;
      if (dy <= 0) {
        // Turned into an upward scroll — hand the gesture back.
        if (offsetRef.current !== 0) setPull(0);
        startY.current = null;
        return;
      }
      const next = pullOffset(dy);
      // Past a few px this is unambiguously our gesture; claim it so the page
      // doesn't scroll underneath the pull.
      if (next > 4 && e.cancelable) e.preventDefault();
      setPull(next);
    };

    const onEnd = () => {
      const fire = willRefresh(offsetRef.current);
      startY.current = null;
      setPull(0);
      if (fire) void refresh.current();
    };

    window.addEventListener("touchstart", onStart, { passive: true });
    window.addEventListener("touchmove", onMove, { passive: false });
    window.addEventListener("touchend", onEnd, { passive: true });
    window.addEventListener("touchcancel", onEnd, { passive: true });
    return () => {
      window.removeEventListener("touchstart", onStart);
      window.removeEventListener("touchmove", onMove);
      window.removeEventListener("touchend", onEnd);
      window.removeEventListener("touchcancel", onEnd);
    };
  }, []);

  // While a refresh runs, hold the content down at the threshold so the
  // spinner has somewhere to live; otherwise follow the finger. A failed
  // check keeps the header open a few seconds longer to say so — the store
  // clears the notice, and the header animates closed with it.
  const shown = refreshing || refreshNotice ? PULL_THRESHOLD : offset;
  const label = refreshing
    ? "Checking…"
    : (refreshNotice ??
      (willRefresh(offset) ? "Release to refresh" : "Pull to refresh"));

  return (
    <>
      {/* The spacer IS the movement — growing it pushes the content down. One
          mechanism, so nothing can double-count the pull. No transition while
          the finger is down (it would lag behind the touch); animate the
          release. */}
      <div
        className="ptr"
        style={{
          height: shown,
          transition: offset === 0 ? "height 220ms ease-out" : "none",
        }}
        aria-live="polite"
        aria-busy={refreshing}
      >
        {shown > 8 && (
          <span className={refreshing ? "ptr-label ptr-spin" : "ptr-label"}>{label}</span>
        )}
      </div>
      {children}
    </>
  );
}
