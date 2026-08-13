// "Allow Vanadium for… [15m] [30m] [1h] [2h] [Until bedtime]"
//
// The popup behind the clock button on every row of the Apps list. One tap picks
// a duration, and that tap IS the decision — it signs and sends. A guardian who
// had to remember a second Save press would be back in the position this feature
// exists to fix.

import { useEffect, useState } from "react";
import { Button } from "./ui";
import {
  HOLD_PRESETS,
  MAX_HOLD_SECONDS,
  holdLabel,
  untilBedtime,
} from "../domain/appHolds";
import type { AppHold, HoldState, Schedule } from "../domain/types";

/**
 * A clock that only runs when it is being read: while the sheet is open, or the
 * Apps list has a live hold to count down. Five seconds is fine because the
 * label rounds to minutes — a per-second tick would redraw the list 300 times
 * for one changed digit.
 *
 * `visibilitychange` is not decoration. The OS freezes an interval the moment
 * this PWA is backgrounded, so without it a guardian returning to the tab reads
 * a countdown frozen at whatever it said when they left — the exact staleness
 * trap that bit us on 2026-07-26.
 */
export function useNowUnix(active: boolean): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    if (!active) return;
    const tick = () => setNow(Math.floor(Date.now() / 1000));
    tick();
    const id = window.setInterval(() => {
      if (!document.hidden) tick();
    }, 5000);
    const onShow = () => {
      if (!document.hidden) tick();
    };
    document.addEventListener("visibilitychange", onShow);
    window.addEventListener("focus", onShow);
    return () => {
      window.clearInterval(id);
      document.removeEventListener("visibilitychange", onShow);
      window.removeEventListener("focus", onShow);
    };
  }, [active]);
  return now;
}

export function AppHoldSheet({
  label,
  direction,
  live,
  nowUnix,
  schedule,
  tooOldNote,
  otherChangesPending,
  busy,
  onPick,
  onEnd,
  onCancel,
}: {
  /** The app's own name — a package id means nothing at a glance. */
  label: string;
  /** What a NEW hold would do: the opposite of what the app is right now. */
  direction: HoldState;
  /** The hold already running on this app, if any. */
  live?: AppHold;
  nowUnix: number;
  /** The ward's schedule, for "Until bedtime". Absent = the preset is hidden. */
  schedule?: Schedule;
  /** Devices reporting a Kintrinsic too old to honour a hold, named. */
  tooOldNote: string | null;
  /** Something else on this screen is unsaved and will travel with this. */
  otherChangesPending: boolean;
  busy: boolean;
  onPick: (untilUnix: number) => void;
  onEnd: () => void;
  onCancel: () => void;
}) {
  const verb = direction === "allowed" ? "Allow" : "Pause";
  const bedtime = untilBedtime(schedule, new Date(nowUnix * 1000));

  return (
    <div
      className="sheet-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={`${verb} ${label} for a while`}
      onClick={() => !busy && onCancel()}
    >
      <div className="sheet" onClick={(e) => e.stopPropagation()}>
        <h2 className="card-title">
          {live ? `${label} — for a while` : `${verb} ${label} for…`}
        </h2>

        {live ? (
          <p className="card-sub" style={{ marginBottom: 14 }}>
            {live.state === "allowed" ? "Open" : "Paused"} for another{" "}
            {holdLabel(live.untilUnix - nowUnix)}. Pick again to change it, or end
            it now and go back to the usual rule.
          </p>
        ) : (
          <p className="card-sub" style={{ marginBottom: 14 }}>
            {direction === "allowed"
              ? "It goes back to blocked on its own — nothing for you to remember."
              : "It comes back on its own — nothing for you to remember."}
          </p>
        )}

        <div
          role="group"
          aria-label="How long"
          style={{ display: "flex", flexWrap: "wrap", gap: 8, marginBottom: 10 }}
        >
          {HOLD_PRESETS.map((p) => (
            <Button
              key={p.minutes}
              variant="secondary"
              disabled={busy}
              // Measured from the clock AT THE PRESS, and never added to what is
              // already left: "1 hour" has to mean the hour the word promises,
              // whatever the ticking `nowUnix` happens to be mid-interval.
              onClick={() => onPick(Math.floor(Date.now() / 1000) + p.minutes * 60)}
              style={{ flex: "1 1 40%", minWidth: 96 }}
            >
              {p.label}
            </Button>
          ))}
          {/* Hidden rather than quietly meaning midnight — a button that says
              "bedtime" and means something else is worse than no button. */}
          {bedtime !== null && bedtime - nowUnix <= MAX_HOLD_SECONDS && (
            <Button
              variant="secondary"
              disabled={busy}
              onClick={() => onPick(bedtime)}
              style={{ flex: "1 1 100%" }}
            >
              Until bedtime ({holdLabel(bedtime - nowUnix)})
            </Button>
          )}
        </div>

        {otherChangesPending && (
          <p className="card-sub" style={{ marginTop: 0 }}>
            Your other unsaved changes on this screen will be saved as well.
          </p>
        )}
        {tooOldNote && (
          <p className="card-sub" style={{ color: "var(--warn, #b45309)" }}>
            {tooOldNote}
          </p>
        )}

        <div className="stack" style={{ marginTop: 6 }}>
          {live && (
            <Button variant="secondary" block disabled={busy} onClick={onEnd}>
              {busy ? "Sending…" : "End now"}
            </Button>
          )}
          <Button variant="secondary" block disabled={busy} onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>
    </div>
  );
}
