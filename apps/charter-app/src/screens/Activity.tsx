import { useMemo } from "react";
import type { ActivityEvent, ActivityOutcome, Child } from "../domain/types";
import { EmptyState, SectionLabel } from "../components/ui";
import { WardHeading, WardTabs } from "../components/wards";
import { goToWard, useWardRoute } from "../domain/wardRoute";
import { useCharter } from "../store/store";
import { WeeklyPicture, type DeviceMeta } from "../insights/WeeklyPicture";
import { outOfHoursLine, weeklyView } from "../insights/usageHistory";
import { unrecognisedGuardTz } from "../domain/unrecognisedTime";
import {
  outOfHoursContributions,
  outOfHoursGuardWeekStart,
  outOfHoursTotals,
} from "../domain/outOfHours";

// Activity — a calm, plain-language history of what happened and when.
// Privacy stance: we only ever show outcomes, never what a child looked at,
// typed, or played. Each row is an icon + colour for the outcome, the friendly
// sentence the store recorded, and a relative time.

type Tone = "ok" | "warn" | "blocked" | "neutral" | "brand";

interface OutcomeStyle {
  emoji: string;
  tone: Tone;
  /** Short word read by screen readers before the sentence. */
  label: string;
}

const OUTCOME_STYLE: Record<ActivityOutcome, OutcomeStyle> = {
  enacted: { emoji: "✓", tone: "ok", label: "Done" },
  thawed: { emoji: "🔓", tone: "ok", label: "Unlocked" },
  "rule-changed": { emoji: "✎", tone: "brand", label: "Change" },
  locked: { emoji: "🔒", tone: "neutral", label: "Locked" },
  denied: { emoji: "✕", tone: "blocked", label: "Not now" },
  failed: { emoji: "!", tone: "warn", label: "Didn't go through" },
  // The break-glass override: warm and visible, never scolding. The ward may
  // have needed it; the point is that it's in the open.
  override: { emoji: "🚨", tone: "warn", label: "Emergency unlock" },
};

const TONE_BG: Record<Tone, string> = {
  ok: "var(--ok-bg)",
  warn: "var(--warn-bg)",
  blocked: "var(--blocked-bg)",
  neutral: "var(--line)",
  brand: "var(--brand-bg)",
};

const TONE_FG: Record<Tone, string> = {
  ok: "var(--ok)",
  warn: "var(--warn)",
  blocked: "var(--blocked)",
  neutral: "var(--text-2)",
  brand: "var(--brand)",
};

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "Just now", "5m ago", "2h ago", "3d ago", then a short date. */
function relativeTime(ts: number, now: number): string {
  const diff = Math.max(0, now - ts);
  if (diff < MINUTE) return "Just now";
  if (diff < HOUR) return `${Math.floor(diff / MINUTE)}m ago`;
  if (diff < DAY) return `${Math.floor(diff / HOUR)}h ago`;
  if (diff < 7 * DAY) return `${Math.floor(diff / DAY)}d ago`;
  return new Date(ts).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/** A friendly day-bucket label: Today / Yesterday / a short date. */
function dayLabel(ts: number, now: number): string {
  const startOf = (t: number) => {
    const d = new Date(t);
    d.setHours(0, 0, 0, 0);
    return d.getTime();
  };
  const days = Math.round((startOf(now) - startOf(ts)) / DAY);
  if (days <= 0) return "Today";
  if (days === 1) return "Yesterday";
  return new Date(ts).toLocaleDateString(undefined, {
    weekday: "long",
    month: "short",
    day: "numeric",
  });
}

interface DayGroup {
  key: string;
  label: string;
  events: ActivityEvent[];
}

function groupByDay(events: ActivityEvent[], now: number): DayGroup[] {
  const groups: DayGroup[] = [];
  let current: DayGroup | null = null;
  for (const ev of events) {
    const label = dayLabel(ev.ts, now);
    if (!current || current.label !== label) {
      current = { key: label + ev.id, label, events: [] };
      groups.push(current);
    }
    current.events.push(ev);
  }
  return groups;
}

function OutcomeIcon({ outcome }: { outcome: ActivityOutcome }) {
  const style = OUTCOME_STYLE[outcome];
  return (
    <span
      aria-hidden="true"
      style={{
        width: 38,
        height: 38,
        flex: "0 0 auto",
        borderRadius: "50%",
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        fontSize: 18,
        lineHeight: 1,
        background: TONE_BG[style.tone],
        color: TONE_FG[style.tone],
      }}
    >
      {style.emoji}
    </span>
  );
}

/** The weekly picture (design memo B1) for one child — their screen-time week,
 *  reflective not punitive. Hidden until the child has a device with a key. */
function ChildWeek({ child, named }: { child: Child; named: boolean }) {
  const { usageHistory, deviceStatus } = useCharter();
  const devices: DeviceMeta[] = child.devices
    .filter((d) => d.devicePubkey)
    .map((d) => ({ machine: d.devicePubkey as string, label: d.label, platform: d.platform }));
  if (devices.length === 0) return null;
  const devicePolicy = child.policies.find((p) => p.scope.kind === "device");
  const allowance = devicePolicy?.budget?.dailyMinutes ?? null;
  const now = Date.now();
  const days = weeklyView(usageHistory, devices.map((d) => d.machine), allowance, now);
  // Out-of-hours (spec 2026-08-03): the week's `alwaysavailable` use,
  // straight off each paired device's own STATUS — Android-only (the clause
  // that produces it doesn't exist on Linux), so a laptop-only child simply
  // never has anything to sum here.
  //
  // FRESHNESS GUARD (review fix C1, 2026-08-04): `outOfHoursWeekSecs` /
  // `outOfHoursNightsWeek` are WEEK-KEYED totals with no date of their own —
  // only the STATUS's `dayKey` says when the report was made. Without
  // dropping a stale or released device first, a phone that reported once
  // late in last week and then went quiet would go on contributing that
  // dead week forever, disagreeing with the ward's own device (whose ledger
  // rolled at the boundary) — the one thing this feature must never do. See
  // `domain/outOfHours.ts` for the day-key mirror of `unrecognisedRows`'s own
  // day guard, and for why `Math.max` on nights makes this guard matter more
  // here than for the plain seconds sum.
  const contributions = outOfHoursContributions(
    child,
    deviceStatus,
    Math.floor(now / 1000),
    unrecognisedGuardTz(devicePolicy),
    outOfHoursGuardWeekStart(devicePolicy),
  );
  const { nights: ohNights, secs: ohSecs } = outOfHoursTotals(contributions);
  const outOfHours = outOfHoursLine(ohNights, ohSecs);
  return (
    <div className="card" style={{ marginBottom: 12 }}>
      {/* Named only when the chart could be anyone's. On one ward's own tab the
          switcher above already says whose week this is, and repeating it in the
          card title is the restatement that made every screen look alike. */}
      {named && (
        <WardHeading
          child={child}
          onOpen={() => goToWard("limits", child.id)}
          openLabel="Their limits"
        />
      )}
      {/* outOfHours is rendered BY WeeklyPicture, beside its own weekSummary
          line, so its empty-state placeholder can stay coherent with it
          (review fix C1, 2026-08-04) — a quiet, informational line, not a
          warning, not a badge, no colour change. Absent entirely for a
          family that never set the always-available clause. */}
      <WeeklyPicture days={days} devices={devices} outOfHours={outOfHours} />
      {!named && (
        <div style={{ marginTop: 10, textAlign: "right" }}>
          {/* The reason to look at a week is usually to change something. */}
          <button
            type="button"
            className="ward-heading-link"
            style={{ marginLeft: 0 }}
            onClick={() => goToWard("limits", child.id)}
          >
            Adjust their limits <span aria-hidden="true">›</span>
          </button>
        </div>
      )}
    </div>
  );
}

/** The sentinel for "no ward chosen" — Activity is the one screen that can
 *  honestly show everybody at once. */
const EVERYONE = "all";

export default function Activity() {
  const { state } = useCharter();
  const now = Date.now();
  // Route-aware, so Home's "how their week is going" can land on one ward's
  // week rather than on a filter the address knew nothing about.
  const [childFilter, setChildFilter] = useWardRoute(
    "activity",
    (id) => state.children.some((c) => c.id === id),
    EVERYONE,
  );

  const hasMultipleChildren = state.children.length > 1;

  const events = useMemo(() => {
    const list =
      childFilter === EVERYONE
        ? state.activity
        : state.activity.filter((e) => e.childId === childFilter);
    // Reverse-chronological; the store prepends newest-first but sort to be safe.
    return [...list].sort((a, b) => b.ts - a.ts);
  }, [state.activity, childFilter]);

  const groups = useMemo(() => groupByDay(events, now), [events, now]);

  const childName = (id: string) =>
    state.children.find((c) => c.id === id)?.name;

  const shownWeeks = (
    childFilter === EVERYONE
      ? state.children
      : state.children.filter((c) => c.id === childFilter)
  ).filter((c) => c.devices.some((d) => d.devicePubkey));

  return (
    <>
      {/* The SAME switcher as Limits, in the same place, with the same gesture.
          It used to be a scrolling strip of filter pills — a third mental model
          for the one act of choosing a ward. */}
      {hasMultipleChildren && (
        <div className="sticky-ward-switcher">
          <WardTabs
            wards={state.children}
            selected={childFilter}
            onSelect={setChildFilter}
            allId={EVERYONE}
            label="Whose activity"
          />
        </div>
      )}

      {shownWeeks.length > 0 && (
        <>
          <SectionLabel>This week</SectionLabel>
          {shownWeeks.map((c) => (
            <ChildWeek key={c.id} child={c} named={childFilter === EVERYONE} />
          ))}
        </>
      )}

      {events.length === 0 ? (
        <EmptyState emoji="🕊️" title="All quiet">
          {childFilter === EVERYONE
            ? "Nothing has happened yet. Activity will show up here as it does."
            : "Nothing yet for this child."}
        </EmptyState>
      ) : (
        groups.map((group) => (
          <section key={group.key} aria-label={group.label}>
            <SectionLabel>{group.label}</SectionLabel>
            <ul
              className="list"
              style={{ listStyle: "none", margin: 0, padding: 0 }}
            >
              {group.events.map((ev) => {
                const who =
                  childFilter === EVERYONE && hasMultipleChildren
                    ? childName(ev.childId)
                    : undefined;
                return (
                  <li key={ev.id} className="row" style={{ cursor: "default" }}>
                    <OutcomeIcon outcome={ev.outcome} />
                    <span className="row-main">
                      <span className="row-title" style={{ fontWeight: 500 }}>
                        <span className="muted" style={{ fontWeight: 700 }}>
                          {OUTCOME_STYLE[ev.outcome].label}.{" "}
                        </span>
                        {ev.summary}
                      </span>
                      <br />
                      <span className="row-sub">
                        {who ? `${who} · ` : ""}
                        <time dateTime={new Date(ev.ts).toISOString()}>
                          {relativeTime(ev.ts, now)}
                        </time>
                      </span>
                    </span>
                  </li>
                );
              })}
            </ul>
          </section>
        ))
      )}
    </>
  );
}
