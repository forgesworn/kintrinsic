import type { CSSProperties } from "react";
import { useEffect, useState } from "react";
import { formatTimeLeft, remainingNow, SECONDS_MATTER_BELOW } from "../domain/timeLeft";
import { GiveTime } from "../components/GiveTime";
import { FinishNow } from "../components/FinishNow";
import { Avatar, Button, Card, Pill, SectionLabel } from "../components/ui";
import { goToWard } from "../domain/wardRoute";
import {
  isSetUp,
  liveStatusFor,
  freshestStatusFor,
  useCharter,
  type CharterState,
} from "../store/store";
import type { Child, ChildRequest, RequestKind } from "../domain/types";
import { liveness, livenessChip } from "../domain/liveness";
import {
  groupExtrasToday,
  groupProgressBuckets,
  groupProgressLine,
  groupProgressRows,
} from "../domain/groupProgress";
import { unrecognisedGuardTz, unrecognisedLine, unrecognisedRows, UNRECOGNISED_EXPLAINER } from "../domain/unrecognisedTime";
import { identityDisplayLabel } from "../domain/launchSignatures";
import { startOfDayUnix } from "../wire/grant";
import Onboarding, { isSetUpEnough } from "./Onboarding";
import CarrierDownload from "../carrier/CarrierDownload";

// Home — the "Today" hub. The first thing a parent sees: one calm card per
// child showing the device we govern, whether they can use it right now, how
// much time is left, and a gentle nudge when there are requests waiting.
//
// The model is device-only (Stage 0/1): a child is a name + a colour + a
// governed computer. There is no child account yet — requests come FROM the
// device, and the parent approves specific apps one at a time (Stage 1).

function go(tab: "limits" | "approvals" | "family"): void {
  window.location.hash = `/${tab}`;
}

/** Open Limits already on THIS ward's tab. Without the id it opened on
 *  whichever child happens to be first, so tapping the second ward silently
 *  showed you the first one's rules — the worst possible screen to be wrong
 *  about. */
function goToLimits(childId: string): void {
  window.location.hash = `/limits/${encodeURIComponent(childId)}`;
}

/** The daily cap (if any) for the child's whole-device limit. */
function dailyCap(child: Child): number | null {
  const device = child.policies.find((p) => p.scope.kind === "device");
  const daily = device?.budget?.paused ? null : device?.budget?.dailyMinutes;
  return daily ?? null;
}

function pendingFor(state: CharterState, childId: string): ChildRequest[] {
  return state.requests.filter(
    (r) => r.childId === childId && r.status === "pending",
  );
}

/** A friendly emoji per kind of ask (mirrors Approvals). */
const KIND_EMOJI: Record<RequestKind, string> = {
  "time.extend": "⏳",
  "install.app": "📦",
  "run.program": "🎮",
  "app.open": "🔓",
};

/** A short, plain-words summary of what the device is asking for. */
function askLabel(req: ChildRequest): string {
  if (req.kind === "time.extend") {
    return `Wants ${req.minutesRequested ?? 15} more minutes`;
  }
  // Belt and braces (F1 review): a raw `cmdline:` string should never itself
  // be device-reported, but resolve it to a real name rather than the
  // needle if it ever rides back on an already-saved policy.
  const what = identityDisplayLabel(req.appLabel, req.appId);
  if (req.kind === "install.app") {
    return `Wants to install ${what ?? "an app"}`;
  }
  if (req.kind === "app.open") {
    return `Wants to open ${what ?? "an app"}`;
  }
  return `Wants to run ${what ?? "a program"}`;
}

/** Stage 1: installs and runs are approved per-app, just this once. */
function isSingleUse(req: ChildRequest): boolean {
  return req.kind === "install.app" || req.kind === "run.program";
}

function TimeBar({ fraction, color }: { fraction: number; color: string }) {
  const pct = Math.max(0, Math.min(1, fraction)) * 100;
  const track: CSSProperties = {
    height: 8,
    borderRadius: 999,
    background: "var(--line)",
    overflow: "hidden",
    marginTop: 14,
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

/** Every governed device, one line each, with whether it is actually THERE.
 *
 *  Was a single line naming the first device and counting the rest ("Pixel 4a
 *  +1 more") beside a HARDCODED "Connected" pill — which claimed a live
 *  connection even with no device paired at all, and said nothing whatever
 *  about the devices it had folded into "+1 more". A guardian glancing at this
 *  card is asking "is their stuff reachable?", and it answered a different
 *  question confidently. */
function DeviceLines({ child }: { child: Child }) {
  const now = Date.now();

  if (child.devices.length === 0) {
    return (
      <div style={deviceBlockStyle}>
        <span className="muted" style={deviceNameStyle}>
          <span aria-hidden="true" style={{ fontSize: 16 }}>
            💻
          </span>
          <span>No device yet</span>
        </span>
        <Pill tone="neutral">Not set up</Pill>
      </div>
    );
  }

  // Tappable, because everything you might want to DO to a device — update it,
  // allow installs, unlock it, disconnect it — lives on Family. Home restated
  // the device's state and then left you to find the other screen yourself.
  return (
    <button
      type="button"
      style={{ ...deviceBlockStyle, ...deviceBlockButtonStyle }}
      onClick={() => go("family")}
      aria-label={`Manage ${child.name}'s devices`}
    >
      {child.devices.map((d) => {
        const chip = livenessChip(
          liveness({ paired: d.pairing === "paired", lastSeenAt: d.lastSeenAt }, now),
          now,
        );
        return (
          <div className="row-between" key={d.id} style={{ gap: 10 }}>
            <span className="muted" style={deviceNameStyle}>
              <span aria-hidden="true" style={{ fontSize: 16 }}>
                {d.platform === "android" ? "📱" : "💻"}
              </span>
              <span
                style={{
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {d.label}
              </span>
            </span>
            <Pill tone={chip.tone}>{chip.text}</Pill>
          </div>
        );
      })}
    </button>
  );
}

const deviceBlockStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  gap: 8,
  marginTop: 16,
  paddingTop: 14,
  borderTop: "1px solid var(--line)",
};

const deviceBlockButtonStyle: CSSProperties = {
  appearance: "none",
  border: 0,
  borderTop: "1px solid var(--line)",
  background: "transparent",
  padding: "14px 0 0",
  width: "100%",
  textAlign: "left",
  color: "inherit",
  font: "inherit",
  cursor: "pointer",
};

const deviceNameStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  minWidth: 0,
  fontSize: "var(--fs-small)",
};

/** The waiting-requests block: a short, Stage-1-framed preview + a CTA. */
function PendingBlock({
  child,
  pending,
}: {
  child: Child;
  pending: ChildRequest[];
}) {
  const n = pending.length;
  return (
    <div
      style={{
        marginTop: 16,
        paddingTop: 16,
        borderTop: "1px solid var(--line)",
      }}
    >
      <div className="card-sub" style={{ marginBottom: 10, fontWeight: 600 }}>
        {n === 1 ? "Waiting for you" : `${n} things waiting for you`}
      </div>

      <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
        {pending.slice(0, 3).map((req) => (
          <li
            key={req.id}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 10,
              marginTop: 8,
            }}
          >
            <span aria-hidden="true" style={{ fontSize: 18, lineHeight: 1 }}>
              {KIND_EMOJI[req.kind]}
            </span>
            <span style={{ minWidth: 0, flex: 1 }}>
              <span style={{ display: "block" }}>{askLabel(req)}</span>
              {isSingleUse(req) && (
                <span className="card-sub">Approve just this once</span>
              )}
            </span>
          </li>
        ))}
      </ul>

      <div style={{ marginTop: 14 }}>
        <Button variant="secondary" block onClick={() => go("approvals")}>
          {n === 1
            ? `Review ${child.name}'s request`
            : `Review ${child.name}'s requests`}
        </Button>
      </div>
    </div>
  );
}

/** Shown for a child whose device isn't connected yet — a gentle setup nudge. */
function NotSetUpBody({ child }: { child: Child }) {
  return (
    <div style={{ marginTop: 16 }}>
      <p className="card-title" style={{ marginBottom: 2 }}>
        Connect a computer to begin
      </p>
      <p className="card-sub" style={{ marginBottom: 14 }}>
        Limits and approvals start working once {child.name}'s computer is
        linked. It takes a minute.
      </p>
      <Button variant="primary" block onClick={() => go("family")}>
        Finish setting up {child.name}
      </Button>
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

function ChildCard({ child }: { child: Child }) {
  const { state, deviceStatus } = useCharter();
  const setUp = isSetUp(child);
  // Prefer the devices' REAL reported state while a fresh heartbeat exists
  // (≤3 min old) — the local guess is only the fallback. The freshest report
  // across the child's paired devices wins.
  const freshest = freshestStatusFor(child, deviceStatus, Date.now());
  const s = liveStatusFor(child, freshest, Date.now());
  const pending = pendingFor(state, child.id);
  // Named times' saved bucket set — the source of both the group progress
  // rows' labels/caps AND (M-2) the day boundary today's granted extras are
  // scoped to (the buckets clause carries its own required tz).
  const devicePolicy = child.policies.find((p) => p.scope.kind === "device");
  const homeBuckets = devicePolicy?.buckets;
  // The device's OWN day-boundary tz — mirrors charterd's `enforcement_tz_of`
  // exactly (schedule, then budget, then UTC — NEVER buckets.tz, unlike
  // `resolveChildTz`) for the unrecognised-time day guard (F5, then New-1).
  const unrecognisedTz = unrecognisedGuardTz(devicePolicy);
  const cap = dailyCap(child);
  const secondsLeft = useLiveSeconds(s.secondsLeftToday, s.asOf);

  const headerBtn: CSSProperties = {
    appearance: "none",
    border: 0,
    background: "transparent",
    padding: 0,
    margin: 0,
    width: "100%",
    display: "flex",
    alignItems: "center",
    gap: 14,
    cursor: "pointer",
    textAlign: "left",
    color: "inherit",
    font: "inherit",
    borderRadius: 12,
  };

  // Header status pill: connection-aware. An unconnected child can't be
  // "Allowed" or "Locked" yet — they just need setting up.
  const headerPill = !setUp ? (
    <Pill tone="neutral">Not set up</Pill>
  ) : (
    <Pill tone={s.allowedNow ? "ok" : "blocked"}>
      {s.allowedNow ? "Allowed now" : "Locked"}
    </Pill>
  );

  const headerSub = !setUp ? "Needs a device" : s.live ? "Live from their device" : "";

  return (
    <Card>
      <button
        type="button"
        style={headerBtn}
        onClick={() => goToLimits(child.id)}
        aria-label={`Open ${child.name}'s limits`}
      >
        <Avatar name={child.name} color={child.color} size={44} />
        <span style={{ flex: 1, minWidth: 0 }}>
          <span
            className="card-title"
            style={{ display: "block", marginBottom: 0 }}
          >
            {child.name}
          </span>
          {headerSub && <span className="card-sub">{headerSub}</span>}
        </span>
        {headerPill}
        <span className="row-chevron" aria-hidden="true">
          ›
        </span>
      </button>

      {!setUp ? (
        <NotSetUpBody child={child} />
      ) : (
        <>
          <div style={{ marginTop: 16 }}>
            {s.allowedNow ? (
              s.minutesLeftToday == null ? (
                <p className="card-title" style={{ marginBottom: 0 }}>
                  No time limit today
                </p>
              ) : (
                <>
                  <div
                    style={{ display: "flex", alignItems: "baseline", gap: 8 }}
                  >
                    <span className="big-number">
                      {formatTimeLeft(secondsLeft ?? s.minutesLeftToday * 60)}
                    </span>
                    <span className="muted">left today</span>
                  </div>
                  {cap != null && (
                    <TimeBar
                      fraction={s.minutesLeftToday / cap}
                      color="var(--ok)"
                    />
                  )}
                </>
              )
            ) : (
              <>
                <p className="card-title" style={{ marginBottom: 2 }}>
                  {s.reason === "budget"
                    ? "Time's up for today"
                    : s.reason === "standdown"
                      ? "Finished for today"
                      : s.nextWindowText ?? "Off for now"}
                </p>
                <p className="card-sub">
                  {s.reason === "budget"
                    ? "They've used all of today's time."
                    : s.reason === "standdown"
                      ? // A stand-down waits for a person — the one reading this.
                        "You asked them to finish up — until you allow them back on."
                      : "Outside of allowed hours right now."}
                </p>
              </>
            )}
          </div>

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
            const rows = unrecognisedRows(child, deviceStatus, Math.floor(Date.now() / 1000), unrecognisedTz);
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

          <DeviceLines child={child} />
          {/* The two levers over today, side by side: add time, or call it.
              No ask required for either — and both work after a "no". */}
          <div style={{ display: "flex", gap: 10, flexWrap: "wrap" }}>
            <div style={{ flex: "1 1 8rem", minWidth: 0 }}>
              <GiveTime child={child} />
            </div>
            <div style={{ flex: "1 1 8rem", minWidth: 0 }}>
              <FinishNow child={child} />
            </div>
          </div>
          {/* Today's number is only half the picture, and the other half is one
              tap away rather than a tab away and then a filter away. */}
          <div style={{ marginTop: 12, textAlign: "right" }}>
            <button
              type="button"
              className="ward-heading-link"
              style={{ marginLeft: 0 }}
              onClick={() => goToWard("activity", child.id)}
            >
              Their week <span aria-hidden="true">›</span>
            </button>
          </div>
        </>
      )}

      {pending.length > 0 && <PendingBlock child={child} pending={pending} />}
    </Card>
  );
}

export default function Home() {
  const { state } = useCharter();
  const setUp = isSetUpEnough(state); // any child has a connected computer
  const hasChildren = state.children.length > 0;

  return (
    <>
      {/* Getting started — self-hides once a computer is connected, at which
          point Home becomes the daily dashboard below. */}
      <Onboarding />

      {hasChildren && (
        <>
          {/* No "N requests are waiting" line here any more. The tab badge
              counts them and each ward's card lists its own — three renderings
              of one fact on one screen was most of why Home felt padded. */}
          <SectionLabel>{setUp ? "Today" : "Your family"}</SectionLabel>
          <div className="stack">
            {state.children.map((child) => (
              <ChildCard key={child.id} child={child} />
            ))}
          </div>
        </>
      )}

      <CarrierDownload />

      {/* The "game & app logins, coming later" teaser is NOT repeated here. It
          lives once, on Family, beside the child it would belong to. */}
    </>
  );
}
