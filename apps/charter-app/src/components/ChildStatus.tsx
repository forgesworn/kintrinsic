import type { CSSProperties } from "react";
import { useEffect, useState } from "react";
import { Button, Pill } from "./ui";
import { LeverSheet } from "./LeverSheet";
import { useGiveSupport, useStopState } from "./leverState";
import { formatTimeLeft, remainingNow, SECONDS_MATTER_BELOW } from "../domain/timeLeft";
import {
  freshestStatusFor,
  isSetUp,
  liveStatusFor,
  useCharter,
} from "../store/store";
import type { Child } from "../domain/types";
import { liveness, livenessChip } from "../domain/liveness";
import { useLocalRoute } from "../carrier/useLocalRoute";
import {
  groupExtrasToday,
  groupProgressBuckets,
  groupProgressLine,
  groupProgressRows,
} from "../domain/groupProgress";
import {
  unrecognisedGuardTz,
  unrecognisedLine,
  unrecognisedRows,
  UNRECOGNISED_EXPLAINER,
} from "../domain/unrecognisedTime";
import { startOfDayUnix, startOfWeekUnix } from "../wire/grant";
import { wardPath } from "../domain/wardRoute";
import { dailyCap } from "./childGlance";

// A child's status right now, and the two levers over today.
//
// Used by Home's child cards and by the top of the child page, so a ward reads
// the same wherever it appears: what the device is doing in plain words, whether
// the devices are reachable, then two equal full-width buttons, "More time" and
// "Stop now" (or "Allow back on"), each opening the one LeverSheet.

function TimeBar({ fraction, color }: { fraction: number; color: string }) {
  const pct = Math.max(0, Math.min(1, fraction)) * 100;
  const track: CSSProperties = {
    height: 8,
    borderRadius: 999,
    background: "var(--line)",
    overflow: "hidden",
    marginTop: 12,
  };
  const fill: CSSProperties = {
    height: "100%",
    width: `${pct}%`,
    background: color,
    borderRadius: 999,
  };
  return (
    <div
      style={track}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
    >
      <div style={fill} />
    </div>
  );
}

/**
 * The remainder, counted down live from the device's last report.
 *
 * Ticks once a second ONLY in the last stretch, where the seconds are what a
 * parent is reading; above that the store's own refresh is plenty and a
 * per-second re-render would just cost battery. The value is recomputed from
 * `Date.now()` rather than accumulated, so the OS freezing this timer while
 * the app is backgrounded costs nothing — it is correct again on the first
 * render after resume.
 */
function useLiveSeconds(
  reported: number | undefined,
  asOf: number | undefined,
): number | undefined {
  const [now, setNow] = useState(() => Date.now());
  const current = remainingNow(reported, asOf, now);
  const ticking = current != null && current < SECONDS_MATTER_BELOW;
  useEffect(() => {
    if (!ticking) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [ticking]);
  return remainingNow(reported, asOf, ticking ? now : Date.now());
}

/** Every governed device on one line, with whether it is actually THERE.
 *
 *  Never a hardcoded "Connected": a guardian glancing at this is asking "is
 *  their stuff reachable?", and a line that claimed a connection with nothing
 *  paired would answer a different question confidently. */
function DeviceSummary({ child }: { child: Child }) {
  const now = Date.now();
  // A device the carrier is talking to directly is on the home network,
  // whatever its last relay heartbeat says.
  const { sessions } = useLocalRoute();

  if (child.devices.length === 0) {
    return (
      <p className="child-devices muted">
        <span aria-hidden="true">💻</span> No device yet
      </p>
    );
  }
  return (
    <p className="child-devices">
      {child.devices.map((d) => {
        const chip = livenessChip(
          liveness({ paired: d.pairing === "paired", lastSeenAt: d.lastSeenAt }, now),
          now,
          d.pairing === "paired" && !!d.devicePubkey && sessions.has(d.devicePubkey),
        );
        return (
          <span
            key={d.id}
            className="child-device"
            title={d.label}
            style={{ color: chip.tone === "warn" ? "var(--warn, #8a5a00)" : undefined }}
          >
            <span aria-hidden="true">{d.platform === "android" ? "📱" : "💻"}</span>{" "}
            <span className="sr-only">{d.label}: </span>
            {chip.text}
          </span>
        );
      })}
    </p>
  );
}

/**
 * `variant="card"` is Home's compact form. `variant="page"` is the child page's
 * "Now" section: the same block plus the lines that explain it (learning time,
 * named-times progress, time the device couldn't attribute).
 */
export function ChildStatus({
  child,
  variant = "card",
}: {
  child: Child;
  variant?: "card" | "page";
}) {
  const { state, deviceStatus } = useCharter();
  const setUp = isSetUp(child);
  const giveSupport = useGiveSupport(child);
  const stop = useStopState(child);
  const [sheet, setSheet] = useState<"give" | "stop" | null>(null);
  // What the last send said, shown under the buttons once the sheet has closed.
  const [note, setNote] = useState<string | null>(null);

  // Prefer the devices' REAL reported state while a fresh heartbeat exists
  // (≤3 min old) — the local guess is only the fallback. The freshest report
  // across the child's paired devices wins.
  const freshest = freshestStatusFor(child, deviceStatus, Date.now());
  const s = liveStatusFor(child, freshest, Date.now());
  const secondsLeft = useLiveSeconds(s.secondsLeftToday, s.asOf);
  const cap = dailyCap(child);

  if (!setUp) {
    return (
      <div className="child-status">
        <p className="card-title" style={{ marginBottom: 2 }}>
          Connect a computer to begin
        </p>
        <p className="card-sub" style={{ marginBottom: 14 }}>
          Limits and approvals start working once {child.name}'s computer is linked. It takes a
          minute.
        </p>
        <Button
          variant="primary"
          block
          className="lever-btn"
          onClick={() => {
            window.location.hash = `/${wardPath(child.id)}/add-device`;
          }}
        >
          Finish setting up {child.name}
        </Button>
      </div>
    );
  }

  // Named times' saved bucket set — the source of both the group progress
  // rows' labels/caps AND (M-2) the day boundary today's granted extras are
  // scoped to (the buckets clause carries its own required tz).
  const devicePolicy = child.policies.find((p) => p.scope.kind === "device");
  const homeBuckets = devicePolicy?.buckets;
  // The device's OWN day-boundary tz — mirrors charterd's `enforcement_tz_of`
  // exactly (schedule, then budget, then UTC — NEVER buckets.tz, unlike
  // `resolveChildTz`) for the unrecognised-time day guard (F5, then New-1).
  const unrecognisedTz = unrecognisedGuardTz(devicePolicy);

  const statusLine = s.allowedNow
    ? s.minutesLeftToday == null
      ? "No time limit today"
      : null // the countdown below
    : s.reason === "budget"
      ? "Time's up for today"
      : s.reason === "standdown"
        ? "Finished for today"
        : s.nextWindowText
          ? `Locked · ${s.nextWindowText}`
          : "Locked · off for now";

  const subLine = s.allowedNow
    ? null
    : s.reason === "budget"
      ? "They've used all of today's time."
      : s.reason === "standdown"
        ? // A stand-down waits for a person — the one reading this.
          "You asked them to finish up — until you allow them back on."
        : "Outside of allowed hours right now.";

  const close = (n?: string) => {
    setSheet(null);
    if (n) setNote(n);
  };

  return (
    <div className="child-status">
      {statusLine ? (
        <>
          <p className="card-title" style={{ marginBottom: 2 }}>
            {statusLine}
          </p>
          {subLine && (variant === "page" || s.reason === "standdown") && (
            <p className="card-sub">{subLine}</p>
          )}
        </>
      ) : (
        <>
          <div style={{ display: "flex", alignItems: "baseline", gap: 8 }}>
            <span className="status-number">
              {formatTimeLeft(secondsLeft ?? (s.minutesLeftToday ?? 0) * 60)}
            </span>
            <span className="muted">left today</span>
          </div>
          {cap != null && s.minutesLeftToday != null && (
            <TimeBar fraction={s.minutesLeftToday / cap} color="var(--ok)" />
          )}
        </>
      )}

      {variant === "page" && s.live && (
        <p className="card-sub" style={{ marginTop: 6 }}>
          <Pill tone="ok">Live from their device</Pill>
        </p>
      )}

      {variant === "page" && (
        <>
          {s.learningMinutesToday != null && (
            <p className="card-sub" style={{ marginTop: 6 }}>
              Learning today: {s.learningMinutesToday} min (time-free)
            </p>
          )}

          {/* Named times' group progress — "Play: 45m of 1h today · 2h of 5h
              this week" — straight from the device's live STATUS, joined
              against the saved policy for labels + caps. Absent STATUS
              groups (a ward whose device predates named times) shows
              nothing new here, never a row of zeros.
              Also joins in what THIS guardian granted today (M-2): STATUS's
              raw meters are the truth of what got spent, but the saved
              policy only knows the BASE cap, so a spend inside a grant the
              guardian personally approved would otherwise read as a breach
              that never happened. */}
          {groupProgressRows(
            freshest?.groups,
            groupProgressBuckets(homeBuckets),
            homeBuckets
              ? groupExtrasToday(
                  state.activity,
                  child.id,
                  startOfDayUnix(Math.floor(Date.now() / 1000), homeBuckets.tz) * 1000,
                )
              : undefined,
            homeBuckets
              ? groupExtrasToday(
                  state.activity,
                  child.id,
                  startOfWeekUnix(
                    Math.floor(Date.now() / 1000),
                    homeBuckets.tz,
                    homeBuckets.weekStart ?? "mon",
                  ) * 1000,
                )
              : undefined,
          ).map((row) => (
            <p key={row.id} className="card-sub" style={{ marginTop: 6 }}>
              {groupProgressLine(row)}
            </p>
          ))}

          {/* Honest attribution's unrecognised-time line (spec §2.3/§2.5): a
              FLOOR, never a total — read the explainer before touching this.
              Renders only when a device actually reported > 0; a device that
              hasn't reported, or genuinely reported zero, gets no row at all,
              never a "0m" line that would read as suspicion over nothing. */}
          {(() => {
            const rows = unrecognisedRows(
              child,
              deviceStatus,
              Math.floor(Date.now() / 1000),
              unrecognisedTz,
            );
            if (rows.length === 0) return null;
            const multiDevice = child.devices.filter((d) => d.pairing === "paired").length > 1;
            return (
              <>
                {rows.map((row) => (
                  <p key={row.deviceId} className="card-sub" style={{ marginTop: 6 }}>
                    {unrecognisedLine(row, multiDevice)}
                  </p>
                ))}
                <p className="card-sub" style={{ marginTop: 2, opacity: 0.8 }}>
                  {UNRECOGNISED_EXPLAINER}
                </p>
              </>
            );
          })()}
        </>
      )}

      <DeviceSummary child={child} />

      {/* The two levers over today, equal and full width: add time, or call it.
          No ask required for either — and both work after a "no". */}
      <div className="lever-row">
        <Button
          variant="secondary"
          className="lever-btn"
          disabled={!giveSupport.canSend}
          onClick={() => {
            setNote(null);
            setSheet("give");
          }}
        >
          More time
        </Button>
        {stop.standing ? (
          <Button
            className="lever-btn"
            disabled={!stop.canSend}
            onClick={() => {
              setNote(null);
              setSheet("stop");
            }}
          >
            Allow back on
          </Button>
        ) : (
          <Button
            variant="secondary"
            className="lever-btn"
            disabled={!stop.canSend}
            onClick={() => {
              setNote(null);
              setSheet("stop");
            }}
          >
            Stop now
          </Button>
        )}
      </div>
      {!giveSupport.canSend && (
        <p className="card-sub" style={{ marginTop: 8 }}>
          {giveSupport.tooOld.map((d) => d.label).join(" and ")} needs the latest Kintrinsic
          before it can accept extra time. Update it, then this turns on by itself.
        </p>
      )}
      {!stop.canSend && (
        <p className="card-sub" style={{ marginTop: 8 }}>
          {stop.tooOld.map((d) => d.label).join(" and ")} needs the latest Kintrinsic before it can
          be asked to finish up. Update it, then this turns on by itself.
        </p>
      )}
      {note && (
        <p className="card-sub" style={{ marginTop: 8 }} aria-live="polite">
          {note}
        </p>
      )}

      {sheet && <LeverSheet mode={sheet} child={child} onClose={close} />}
    </div>
  );
}
