import { endOfTodayUnix, TETHER_WINDOWS, tetherWindowLabel } from "../domain/tetherWindow";
import { deviceScopeNote } from "../domain/deviceWords";
import {
  appsByDevice,
  appsBySection,
  appsFor,
  mergedApps,
  type AppSection,
} from "../domain/deviceApps";
import { deviceSetLimits } from "../domain/standing";
import { LEARNING_CATALOGUE } from "../data/learning_catalogue";
import { normalizeWebDomain } from "../domain/webDomain";
import {
  deliverableDevices,
  supportNote,
  tooOldNote,
  unsupportedNote,
  wardenSupport,
  type WardenFeature,
} from "../domain/wardenSupport";
import {
  holdDirection,
  holdFor,
  holdRowLabel,
  pruneHolds,
  putHold,
} from "../domain/appHolds";
import { AppHoldSheet, useNowUnix } from "../components/AppHoldSheet";
import {
  isControlSplit,
  pickOverrides,
  setControlShared,
  setControlSplit,
  splitDeviceIds,
  type PolicyOverride,
  type SplittableControl,
} from "../domain/effectivePolicy";
import {
  siteClosureFor,
  siteClosureMessage,
  siteIdFor,
} from "../domain/siteClosure";
import {
  applyAppsFragment,
  decompose,
  groupsToClauses,
  namedTimesError,
  stripManagedApps,
  type NamedGroup,
} from "../domain/namedTimes";
import { NamedTimesSection } from "./NamedTimes";
import { humanLabelFor, isCmdlineIdentity } from "../domain/launchSignatures";
import { useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import {
  Banner,
  Button,
  Card,
  EmptyState,
  Pill,
  SectionLabel,
} from "../components/ui";
import { Section, SectionList, SubTabs } from "../components/Section";
import { WardHeading, WardTabs } from "../components/wards";
import { useWardRoute } from "../domain/wardRoute";
import { freshestStatusFor, liveStatusFor, useCharter } from "../store/store";
import {
  applyToEveryDay,
  collapseSourceDay,
  isSameEveryDay,
  sharedWindows,
} from "../domain/everyDay";
import {
  WEEKDAYS,
  type Budget,
  type Child,
  type Device,
  type Policy,
  type Schedule,
  type ScheduleWindow,
  type Weekday,
  type WebPolicy,
  type AppsPolicy,
  type LearningAppSel,
  type ListeningPolicy,
  type Lifeline,
  type Tethering,
  type BucketsPolicy,
  type AlwaysAvailablePolicy,
  type TimeModel,
} from "../domain/types";

// ---------------------------------------------------------------------------
// Small local helpers (used only by this screen)
// ---------------------------------------------------------------------------

const LOCAL_TZ =
  (typeof Intl !== "undefined" &&
    Intl.DateTimeFormat().resolvedOptions().timeZone) ||
  "America/New_York";

const DAY_FULL: Record<Weekday, string> = {
  mon: "Monday",
  tue: "Tuesday",
  wed: "Wednesday",
  thu: "Thursday",
  fri: "Friday",
  sat: "Saturday",
  sun: "Sunday",
};

const DEFAULT_WINDOW: ScheduleWindow = { start: "16:00", end: "18:00" };

function fmtDuration(mins: number): string {
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  if (h && m) return `${h}h ${m}m`;
  if (h) return `${h}h`;
  return `${m}m`;
}

/** Normalize a saved Schedule into a stable, fully-populated draft shape. */
export function buildSchedule(src?: Schedule): Schedule {
  const weekly: Partial<Record<Weekday, ScheduleWindow[]>> = {};
  for (const d of WEEKDAYS) {
    weekly[d] = (src?.weekly?.[d] ?? []).map((w) => ({
      start: w.start,
      end: w.end,
    }));
  }
  const out: Schedule = {
    tz: src?.tz ?? LOCAL_TZ,
    weekly,
    paused: src?.paused ?? false,
  };
  if (src?.overrides) out.overrides = src.overrides;
  return out;
}

/** Normalize a saved Budget into a stable draft shape. */
function buildBudget(src?: Budget): Budget {
  return {
    tz: src?.tz ?? LOCAL_TZ,
    dailyMinutes: src?.dailyMinutes ?? null,
    weeklyMinutes: src?.weeklyMinutes ?? null,
    weekStart: src?.weekStart ?? "sun",
    paused: src?.paused ?? false,
    // Explicit, never left undefined, same reason `paused` is normalised
    // above: draft and saved are both built through this function, so an
    // untouched tier never reads dirty — but the WIRE representation of
    // "session" stays absence (`budgetToGrant` only emits `model` for
    // `"named"`), never this explicit default.
    model: src?.model ?? "session",
  };
}

/**
 * "Off unless I open it" — the posture for a device that is nobody's daily
 * driver (the spare family tablet). DERIVED, never stored: a schedule that
 * allows no time anywhere, and isn't app-paused, IS dormant. A stored flag
 * would be a second source of truth able to disagree with the clause actually
 * on the wire, and wouldn't survive the schedule being edited elsewhere.
 *
 * `scheduleToGrant` encodes exactly this state as wire `paused: true` (block
 * all), so the device is off with no standing allowance to discover — and a
 * guardian's "Give time" opens it for that long and no longer.
 */
export function isDormant(s: Schedule): boolean {
  if (s.paused) return false;
  if (s.overrides && Object.values(s.overrides).some((w) => (w ?? []).length > 0)) return false;
  return WEEKDAYS.every((d) => (s.weekly[d] ?? []).length === 0);
}

/** Put a device to sleep: no allowed time anywhere, and never app-paused (which
 *  would mean the opposite — always allowed). One-off override days go too, or
 *  a forgotten one would wake the device on a date nobody remembers setting.
 *
 *  The week is spelled out as seven EMPTY days rather than `{}` so this is
 *  byte-identical to what `buildSchedule` reconstructs after saving. Returning
 *  `{}` left the draft permanently unequal to the saved policy, so the card
 *  stayed dirty and "Save changes" never greyed out — it looked like the save
 *  hadn't taken (decented, 2026-07-29). */
export function toDormant(s: Schedule): Schedule {
  const weekly: Schedule["weekly"] = {};
  for (const d of WEEKDAYS) weekly[d] = [];
  return { tz: s.tz, weekly, paused: false };
}

/** Wake it: a plain allowed window every day. Returning to a BLANK week would
 *  leave the guardian staring at an editor that still means "off", with no way
 *  to tell the toggle had done anything. */
export function fromDormant(s: Schedule): Schedule {
  const weekly: Schedule["weekly"] = {};
  for (const d of WEEKDAYS) weekly[d] = [{ ...DEFAULT_WINDOW }];
  return { ...s, weekly, paused: false };
}

function scheduleSummary(s: Schedule, authored = true): string {
  // Never claim a posture over a ward whose schedule was never signed. "Off"
  // there would be the guardian's screen asserting a device is held when
  // nothing is holding it — the one failure this whole card exists to avoid.
  if (!authored) return "No schedule set yet";
  if (s.paused) return "Schedule off — allowed anytime";
  const open = WEEKDAYS.filter((d) => (s.weekly[d] ?? []).length > 0).length;
  // Not "No time allowed yet" — that reads as a setup the guardian abandoned
  // half-done, when it is in fact the whole point of a spare device.
  if (open === 0) return "Off — you open it when needed";
  if (open === 7) return "Allowed every day";
  return `Allowed ${open} ${open === 1 ? "day" : "days"} a week`;
}

/**
 * First allowed-time window that isn't a valid same-day span, if any. The wire
 * format has no midnight-crossing window (`assertWindows` in wire/clause.ts
 * rejects start >= end), so an overnight range like 22:00–02:00 is caught here —
 * in the editor, with a friendly message — rather than throwing silently at sign
 * time or being accepted locally and then reported "locked" for a day the
 * summary calls "Allowed". Returns null when the schedule is valid (or paused).
 */
function scheduleWindowError(s: Schedule): string | null {
  if (s.paused) return null;
  const HHMM = /^([01]\d|2[0-3]):([0-5]\d)$/;
  for (const day of WEEKDAYS) {
    for (const w of s.weekly[day] ?? []) {
      if (!HHMM.test(w.start) || !HHMM.test(w.end) || w.start >= w.end) {
        return `${DAY_FULL[day]}: an allowed time must start before it ends (overnight ranges that cross midnight aren’t supported yet — split them across two days).`;
      }
    }
  }
  return null;
}

function budgetSummary(b: Budget): string {
  if (b.paused || b.dailyMinutes == null) return "No daily limit";
  return `${fmtDuration(b.dailyMinutes)} a day`;
}

/** Normalize a saved WebPolicy into a stable draft shape (filtering off by default). */
function buildWeb(src?: WebPolicy): WebPolicy {
  return {
    enabled: src?.enabled ?? false,
    posture: src?.posture ?? "blocklist",
    ageTier: src?.ageTier ?? "older",
    allow: src?.allow ? [...src.allow] : [],
    block: src?.block ? [...src.block] : [],
    youtube: src?.youtube ?? "off",
    safeSearch: src?.safeSearch ?? true,
  };
}

function webSummary(w: WebPolicy): string {
  if (!w.enabled) return "Web filtering off — all sites allowed";
  const bits: string[] = [w.posture === "allowlist" ? "Only allowed sites" : "All but blocked"];
  if (w.block.length) bits.push(`${w.block.length} blocked`);
  if (w.allow.length) bits.push(`${w.allow.length} allowed`);
  if (w.youtube !== "off") bits.push(`YouTube ${w.youtube}`);
  return bits.join(" · ");
}

/** Normalize saved lifeline numbers into a stable draft (empty by default). */
export function buildLifeline(src?: Lifeline): Lifeline {
  return {
    numbers: (src?.numbers ?? []).map((n) => ({ label: n.label, number: n.number })),
    emergencyServices: src?.emergencyServices ?? false,
    torch: src?.torch ?? false,
    // ON unless a guardian has explicitly turned it off — matching the device,
    // which treats an absent break-glass config as the safety net being up
    // (`BreakGlassCfg::safety_net`). A guardian who never opens this section
    // must not thereby leave a ward with no way out of a lock the device can't
    // reach the relay to lift.
    breakGlass: src?.breakGlass ?? { enabled: true, scope: "full", durationMinutes: 10 },
  };
}

/** The wire cap (device validates 1..5 fail-closed — mirror it here). */
const MAX_LIFELINE_NUMBERS = 5;
/** The ward-app versionCode that ACCEPTS the v2 lifeline shapes. A device
 *  below this drops a 4+ number / v2 clause wholesale (fail-closed), which
 *  would leave the ward with NO lifeline at all — so the editor stays capped
 *  at the v1 shape until every paired phone has updated. Self-lifting. */
const LIFELINE_V2_MIN_VERSION_CODE = 14;

/** Client-side mirror of the device's fail-closed number check — the device
 *  drops a malformed body wholesale, so catch it here where it can be fixed. */
export function lifelineNumberError(l: Lifeline): string | null {
  const rows = l.numbers.filter((n) => n.label.trim() || n.number.trim());
  // With no numbers there is no lifeline clause at all (the device drops a
  // numberless body fail-closed, so we never send one) — and every OTHER
  // lock-screen setting rides that same clause. Without this check a torch /
  // emergency-number / break-glass change "saved" cleanly and reached nobody.
  if (rows.length === 0)
    return "These lock-screen settings travel with the lifeline numbers — add at least one number they can call, then save.";
  if (rows.length > MAX_LIFELINE_NUMBERS) return "Up to five lifeline numbers.";
  for (const n of rows) {
    if (!n.label.trim() || n.label.trim().length > 20)
      return "Each lifeline number needs a short name (up to 20 letters).";
    if (!/^[+0-9][0-9 ()-]{4,19}$/.test(n.number.trim()))
      return `“${n.number}” doesn’t look like a phone number.`;
  }
  return null;
}

/** Normalize a saved Tethering posture into a stable draft (blocked by default). */
function buildTethering(src?: Tethering): Tethering {
  return {
    allow: src?.allow ?? "none",
    ...(src?.until != null ? { until: src.until } : {}),
  };
}

function tetheringSummary(t: Tethering): string {
  // "allowed", not "on": the clause is permission — the child turns the hotspot
  // on at their end when they need it (and it switches itself off after that).
  switch (t.allow) {
    case "raw":
      return "Hotspot allowed (guests unfiltered)";
    case "filtered":
      return "Filtered hotspot allowed";
    default:
      return "Hotspot off";
  }
}

/** Normalize a saved listening agreement. Defaults to `stop` — the behaviour
 *  before this setting existed, and what the device does with no clause. */
export function buildListening(src?: ListeningPolicy): ListeningPolicy {
  return {
    mode: src?.mode ?? "stop",
    graceMinutes: src?.graceMinutes ?? 30,
    apps: [...(src?.apps ?? [])],
  };
}

export function listeningSummary(l: ListeningPolicy): string {
  if (l.mode === "stop" || l.apps.length === 0) return "Audio stops with the screen";
  return l.mode === "continue"
    ? "Audio may keep playing"
    : `Audio may finish (${l.graceMinutes ?? 30} min)`;
}

/** Normalize a saved always-available list. Defaults to empty — the behaviour
 *  before this clause existed. */
export function buildAlwaysAvailable(src?: AlwaysAvailablePolicy): AlwaysAvailablePolicy {
  return {
    apps: (src?.apps ?? []).map((a) =>
      a.untilUnix === undefined ? { pkg: a.pkg } : { pkg: a.pkg, untilUnix: a.untilUnix },
    ),
  };
}

/** A lapsed entry is already inert on the device; never let the summary claim
 *  a grant that has ended. */
export function alwaysAvailableSummary(a: AlwaysAvailablePolicy, now: number): string {
  const live = a.apps.filter((x) => x.untilUnix === undefined || x.untilUnix > now);
  const standing = live.filter((x) => x.untilUnix === undefined).length;
  const temporary = live.length - standing;
  if (live.length === 0) return "Nothing — the lock takes everything";
  const parts: string[] = [];
  if (standing > 0) parts.push(`${standing} app${standing === 1 ? "" : "s"}, always`);
  if (temporary > 0) parts.push(`${temporary} for now`);
  return parts.join(" · ");
}

/** Normalize a saved AppsPolicy into a stable draft shape (off by default).
 *
 *  Holds are copied VERBATIM, never pruned against the clock. Pruning here would
 *  make the draft and the saved policy diverge as time passed — the draft is
 *  built once at mount, the saved one is rebuilt whenever the policy changes —
 *  and the section would show unsaved changes that nobody made. Expired holds are
 *  hidden at render and dropped at publish instead, where the clock belongs. */
export function buildApps(src?: AppsPolicy): AppsPolicy {
  const apps: AppsPolicy = {
    enabled: src?.enabled ?? false,
    posture: src?.posture ?? "blocklist",
    blocked: src?.blocked ? [...src.blocked] : [],
    allowed: src?.allowed ? [...src.allowed] : [],
  };
  if (src?.holds?.length) apps.holds = src.holds.map((h) => ({ ...h }));
  // Named times' on-request groups are the only thing that ever populates
  // this (see `applyAppsFragment`) — dropping it here made `appsDirty` (which
  // compares against this normalized shape) permanently stuck true the
  // moment an on-request group existed: the save round-tripped fine, but the
  // Apps row never settled. Found live-testing this feature.
  if (src?.askFirst?.length) apps.askFirst = [...src.askFirst];
  // Same reason as `askFirst` above: this normalized shape is what `appsDirty`
  // compares the draft against, so a dimension dropped here reads as an unsaved
  // change forever the moment it has a value. Absent when empty, never `[]`.
  if (src?.hidden?.length) apps.hidden = [...src.hidden];
  return apps;
}

/**
 * The collapsed Apps row's summary. Under allowlist posture, the "allowed"
 * count must match what the WIRE actually signs, not the raw stored list:
 * `appsToGrant` strips any `askFirst` pkg out of the signed `allowed` (an
 * on-request pkg is never also unconditionally allowed — see
 * `wire/clause.ts`'s N1 fix), so an on-request pkg that still happens to sit
 * in `a.allowed` (the stored policy, deliberately never mutated — see
 * `applyAppsFragment`'s doc) must NOT be counted here either. Without this,
 * the collapsed Apps row and the Named times row disagreed about the same
 * pkg at once — "Only 2 apps allowed" while Named times called it "on
 * request" (found in review): the wire/enforcement were correct, only this
 * summary was reading the pre-strip count.
 */
export function appsSummary(a: AppsPolicy, nowUnix: number): string {
  // Removed apps are counted whatever app CONTROL is doing — the clause emits
  // them even when the policy is paused (see `appsToGrant`), so a collapsed
  // row that said a flat "No app blocks" for a tablet with forty apps taken
  // off it would be hiding the only thing that section had actually done.
  const removed = a.hidden?.length ?? 0;
  const alsoRemoved = removed ? `, ${removed} removed` : "";
  if (!a.enabled) {
    return removed ? `No app blocks, ${removed} removed from device` : "No app blocks";
  }
  const held = pruneHolds(a.holds, nowUnix).length;
  const forAWhile = held ? `, ${held} for a while` : "";
  if (a.posture === "allowlist") {
    const askFirst = new Set(a.askFirst ?? []);
    const allowedCount = a.allowed.filter((p) => !askFirst.has(p)).length;
    return `Only ${allowedCount} app${allowedCount === 1 ? "" : "s"} allowed${forAWhile}${alsoRemoved}`;
  }
  return `${a.blocked.length} app${a.blocked.length === 1 ? "" : "s"} blocked${forAWhile}${alsoRemoved}`;
}

/** The collapsed-row summary for Named times — a closed row has to say what
 *  it currently means, or collapsing the screen just hides the rules instead
 *  of summarising them. Counts groups by policy rather than trying to name
 *  them all, mirroring the old buckets/learning summaries' "say the shape,
 *  not every detail" approach. */
export function namedTimesSummary(groups: NamedGroup[]): string {
  if (groups.length === 0) return "No named times yet";
  const free = groups.filter((g) => g.policy === "free").length;
  const counted = groups.filter((g) => g.policy === "counted").length;
  const onRequest = groups.filter((g) => g.policy === "onRequest").length;
  const bits: string[] = [];
  if (free) bits.push(`${free} free`);
  if (counted) bits.push(`${counted} counted`);
  if (onRequest) bits.push(`${onRequest} on request`);
  return bits.join(", ");
}

export function lifelineSummary(l: Lifeline): string {
  const n = l.numbers.filter((x) => x.label.trim() && x.number.trim()).length;
  const bits: string[] = [
    n === 0 ? "No numbers set" : `${n} number${n === 1 ? "" : "s"}`,
  ];
  if (l.emergencyServices) bits.push("emergency services");
  // Absent means ON, matching the device (`BreakGlassCfg::safety_net`) — a
  // summary that read "off" for a ward who has never opened this section would
  // be the screen lying about the one thing that gets them out of a lock.
  if (l.breakGlass?.enabled !== false) bits.push("emergency unlock");
  return bits.join(" · ");
}

// ---------------------------------------------------------------------------
// How the nine controls are grouped
// ---------------------------------------------------------------------------

/** The second level of tabs. Was one flat run of nine always-open editors —
 *  about 2,000px of controls with Save at the very bottom. */
export type SectionTab = "time" | "content" | "safety";

export type SectionId =
  | "schedule"
  | "budget"
  | "named-times"
  | "listening"
  | "web"
  | "apps"
  | "always-available"
  | "lifeline"
  | "tethering";

export const SECTION_TITLE: Record<SectionId, string> = {
  schedule: "Daily schedule",
  budget: "Time limit",
  "named-times": "Named times",
  listening: "When time's up",
  web: "Websites",
  apps: "Apps",
  "always-available": "Always available",
  lifeline: "Lifeline",
  tethering: "Hotspot",
};

export const SECTION_TAB_OF: Record<SectionId, SectionTab> = {
  schedule: "time",
  budget: "time",
  "named-times": "time",
  listening: "time",
  web: "content",
  apps: "content",
  "always-available": "content",
  lifeline: "safety",
  tethering: "safety",
};

export const SECTION_TABS: { id: SectionTab; label: string }[] = [
  { id: "time", label: "Time" },
  { id: "content", label: "Apps & web" },
  { id: "safety", label: "Safety" },
];

/** Which tab a section lives on — so the save bar can send you to an unsaved
 *  change that is sitting on a tab you can't currently see. */
export function tabOfSection(id: string): SectionTab {
  return SECTION_TAB_OF[id as SectionId] ?? "time";
}

export function titleOfSection(id: string): string {
  return SECTION_TITLE[id as SectionId] ?? id;
}

// ---------------------------------------------------------------------------
// Tiny presentational controls (inline-styled so we touch no shared CSS)
// ---------------------------------------------------------------------------

export function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      style={{
        width: 46,
        height: 28,
        flex: "0 0 auto",
        borderRadius: 999,
        border: "1px solid var(--line)",
        background: checked ? "var(--ok)" : "var(--line)",
        position: "relative",
        cursor: "pointer",
        padding: 0,
        transition: "background-color 0.15s ease",
      }}
    >
      <span
        aria-hidden="true"
        style={{
          position: "absolute",
          top: 2,
          left: checked ? 20 : 2,
          width: 22,
          height: 22,
          borderRadius: "50%",
          background: "#fff",
          boxShadow: "var(--shadow-1)",
          transition: "left 0.15s ease",
        }}
      />
    </button>
  );
}

const roundBtn: CSSProperties = {
  width: 44,
  height: 44,
  flex: "0 0 auto",
  borderRadius: "50%",
  border: "1px solid var(--line)",
  background: "var(--surface)",
  color: "var(--text)",
  fontSize: 22,
  fontWeight: 700,
  lineHeight: 1,
  cursor: "pointer",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
};

export function Stepper({
  value,
  min,
  max,
  step,
  onChange,
  label,
}: {
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (next: number) => void;
  label: string;
}) {
  return (
    <div
      style={{ display: "flex", alignItems: "center", gap: 14 }}
      role="group"
      aria-label={label}
    >
      <button
        type="button"
        style={{ ...roundBtn, opacity: value <= min ? 0.4 : 1 }}
        aria-label={`Less — ${label}`}
        disabled={value <= min}
        onClick={() => onChange(Math.max(min, value - step))}
      >
        −
      </button>
      <span
        aria-live="polite"
        style={{
          minWidth: 96,
          textAlign: "center",
          fontWeight: 700,
          fontSize: 18,
        }}
      >
        {fmtDuration(value)}
      </span>
      <button
        type="button"
        style={{ ...roundBtn, opacity: value >= max ? 0.4 : 1 }}
        aria-label={`More — ${label}`}
        disabled={value >= max}
        onClick={() => onChange(Math.min(max, value + step))}
      >
        +
      </button>
    </div>
  );
}

const timeInput: CSSProperties = {
  minHeight: 44,
  padding: "0 10px",
  border: "1px solid var(--line)",
  borderRadius: "var(--radius-control)",
  background: "var(--surface)",
  color: "var(--text)",
  flex: "1 1 0",
  minWidth: 0,
};

// ---------------------------------------------------------------------------
// Schedule editor
// ---------------------------------------------------------------------------

function ScheduleEditor({
  schedule,
  onChange,
}: {
  schedule: Schedule;
  onChange: (next: Schedule) => void;
}) {
  function setWindows(day: Weekday, windows: ScheduleWindow[]) {
    onChange({ ...schedule, weekly: { ...schedule.weekly, [day]: windows } });
  }

  // Most families start from one rule for the whole week and only then carve
  // out a different Saturday, so the editor opens on whichever view matches the
  // charter as it actually stands — never a stored preference that could show a
  // single row for a week that genuinely differs.
  const [perDay, setPerDay] = useState(() => !isSameEveryDay(schedule));
  const [confirmCollapse, setConfirmCollapse] = useState(false);
  const shared = sharedWindows(schedule);
  const sharedAllowed = shared.length > 0;

  function setSharedWindows(windows: ScheduleWindow[]) {
    onChange(applyToEveryDay(schedule, windows));
  }

  const collapse = () => {
    setPerDay(false);
    setConfirmCollapse(false);
    if (!isSameEveryDay(schedule)) onChange(applyToEveryDay(schedule, shared));
  };

  const modeSwitch = (
    <div style={{ marginBottom: 4 }}>
      <div className="row-between">
        <span className="row-sub">Set each day separately</span>
        <Toggle
          checked={perDay}
          label="Set each day separately"
          onChange={(on) => {
            if (on) {
              // Widening never loses anything: the days already agree.
              setPerDay(true);
              setConfirmCollapse(false);
            } else if (isSameEveryDay(schedule)) {
              collapse();
            } else {
              // Collapsing WOULD overwrite days that differ, so ask first and
              // name what will happen rather than quietly flattening the week.
              setConfirmCollapse(true);
            }
          }}
        />
      </div>
      {confirmCollapse && (
        <div style={{ marginTop: 8 }}>
          <Banner tone="warn">
            Use {DAY_FULL[collapseSourceDay(schedule)]}&rsquo;s times for every
            day? The days you&rsquo;ve set differently will be replaced.
          </Banner>
          <div style={{ display: "flex", gap: 10, marginTop: 8 }}>
            <Button onClick={collapse}>Use the same times</Button>
            <Button variant="secondary" onClick={() => setConfirmCollapse(false)}>
              Keep them different
            </Button>
          </div>
        </div>
      )}
    </div>
  );

  if (!perDay) {
    return (
      <div className="stack" aria-disabled={schedule.paused}>
        {modeSwitch}
        <div
          style={{
            borderTop: "1px solid var(--line)",
            paddingTop: 14,
            opacity: schedule.paused ? 0.5 : 1,
          }}
        >
          <div className="row-between">
            <span style={{ fontWeight: 600 }}>Every day</span>
            <span style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <span className="row-sub">{sharedAllowed ? "Allowed" : "Blocked"}</span>
              <Toggle
                checked={sharedAllowed}
                label="Allow screen time every day"
                onChange={(on) => setSharedWindows(on ? [{ ...DEFAULT_WINDOW }] : [])}
              />
            </span>
          </div>

          {sharedAllowed && !schedule.paused && (
            <div className="stack" style={{ marginTop: 12 }}>
              {shared.map((w, i) => (
                <div key={i} style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  <input
                    type="time"
                    style={timeInput}
                    value={w.start}
                    aria-label="Start time, every day"
                    onChange={(e) =>
                      setSharedWindows(
                        shared.map((x, xi) =>
                          xi === i ? { ...x, start: e.target.value } : x,
                        ),
                      )
                    }
                  />
                  <span className="muted">to</span>
                  <input
                    type="time"
                    style={timeInput}
                    value={w.end}
                    aria-label="End time, every day"
                    onChange={(e) =>
                      setSharedWindows(
                        shared.map((x, xi) =>
                          xi === i ? { ...x, end: e.target.value } : x,
                        ),
                      )
                    }
                  />
                  <button
                    type="button"
                    style={{ ...roundBtn, fontSize: 18 }}
                    aria-label="Remove this time"
                    onClick={() => setSharedWindows(shared.filter((_, xi) => xi !== i))}
                  >
                    ×
                  </button>
                </div>
              ))}
              <Button
                variant="ghost"
                onClick={() => setSharedWindows([...shared, { ...DEFAULT_WINDOW }])}
              >
                + Add another time
              </Button>
            </div>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="stack" aria-disabled={schedule.paused}>
      {modeSwitch}
      {WEEKDAYS.map((day) => {
        const windows = schedule.weekly[day] ?? [];
        const allowed = windows.length > 0;
        return (
          <div
            key={day}
            style={{
              borderTop: "1px solid var(--line)",
              paddingTop: 14,
              opacity: schedule.paused ? 0.5 : 1,
            }}
          >
            <div className="row-between">
              <span style={{ fontWeight: 600 }}>{DAY_FULL[day]}</span>
              <span style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <span className="row-sub">
                  {allowed ? "Allowed" : "Blocked"}
                </span>
                <Toggle
                  checked={allowed}
                  label={`Allow screen time on ${DAY_FULL[day]}`}
                  onChange={(on) =>
                    setWindows(day, on ? [{ ...DEFAULT_WINDOW }] : [])
                  }
                />
              </span>
            </div>

            {allowed && !schedule.paused && (
              <div className="stack" style={{ marginTop: 12 }}>
                {windows.map((w, i) => (
                  <div
                    key={i}
                    style={{ display: "flex", alignItems: "center", gap: 8 }}
                  >
                    <input
                      type="time"
                      style={timeInput}
                      value={w.start}
                      aria-label={`${DAY_FULL[day]} start time`}
                      onChange={(e) =>
                        setWindows(
                          day,
                          windows.map((x, xi) =>
                            xi === i ? { ...x, start: e.target.value } : x,
                          ),
                        )
                      }
                    />
                    <span className="muted">to</span>
                    <input
                      type="time"
                      style={timeInput}
                      value={w.end}
                      aria-label={`${DAY_FULL[day]} end time`}
                      onChange={(e) =>
                        setWindows(
                          day,
                          windows.map((x, xi) =>
                            xi === i ? { ...x, end: e.target.value } : x,
                          ),
                        )
                      }
                    />
                    <button
                      type="button"
                      style={{ ...roundBtn, fontSize: 18 }}
                      aria-label={`Remove this time on ${DAY_FULL[day]}`}
                      onClick={() =>
                        setWindows(
                          day,
                          windows.filter((_, xi) => xi !== i),
                        )
                      }
                    >
                      ×
                    </button>
                  </div>
                ))}
                <Button
                  variant="ghost"
                  onClick={() =>
                    setWindows(day, [...windows, { ...DEFAULT_WINDOW }])
                  }
                >
                  + Add another time
                </Button>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Budget editor
// ---------------------------------------------------------------------------

function BudgetEditor({
  budget,
  onChange,
  onDevice,
  timeModelNote,
}: {
  budget: Budget;
  onChange: (next: Budget) => void;
  /** Limits this ward's own device is enforcing, set on the machine rather
   *  than from here. Shown so the screen tells the truth, and so the guardian
   *  is warned BEFORE replacing them — precedence is winner-takes-all. */
  onDevice?: { dailyMinutes?: number; weeklyMinutes?: number } | null;
  /** `unsupportedNote`'s sentence for the `timeModel` feature, or `null` when
   *  every paired device honours it — computed by the parent (it alone knows
   *  the ward's devices) and rendered here, beside the selector it's about,
   *  rather than floated somewhere else on the screen. Only shown while
   *  `named` is actually selected: the gap is about what happens WHEN you
   *  choose that tier, so it would be noise on `session` (which every
   *  platform has always honoured). */
  timeModelNote?: string | null;
}) {
  const dailyOn = budget.dailyMinutes != null;
  const weeklyOn = budget.weeklyMinutes != null;
  const model: TimeModel = budget.model ?? "session";

  return (
    <div className="stack">
      {onDevice && (
        <Banner tone="info">
          {onDevice.dailyMinutes != null
            ? `Their computer is already set to ${fmtDuration(onDevice.dailyMinutes)} a day, set up on the machine itself.`
            : "Their computer already has limits set up on the machine itself."}{" "}
          Turning on a limit here <b>replaces</b> what the computer was told —
          it doesn't add to it.
        </Banner>
      )}

      {/* Time model (2026-08-06 "named costs" design): WHAT the budget below
          is actually spent on. Two tiers only — "hours" (schedule with no
          budget clause at all) already exists without a name on the wire,
          and doesn't belong in this selector. */}
      <div className="field" style={{ margin: 0 }}>
        <span className="field-label">How time is spent</span>
        <Segmented<TimeModel>
          value={model}
          options={[
            { value: "session", label: "Time on the device" },
            { value: "named", label: "Only certain apps" },
          ]}
          onChange={(v) => onChange({ ...budget, model: v })}
        />
        <p className="card-sub" style={{ marginTop: 6 }}>
          {model === "session"
            ? "Two hours a day, whatever they're doing."
            : "The laptop is free. You choose what costs."}
        </p>
      </div>

      {model === "named" && (
        <>
          <Banner tone="info">
            Nothing costs until you name it in <b>Named times</b>. Several
            named apps open together still only spend the day once between
            them. An app you haven't named — including one just
            installed — is free until you name it.
          </Banner>
          {timeModelNote && <ParityNote note={timeModelNote} />}
        </>
      )}

      {/* Daily limit */}
      <div className="row-between">
        <div className="row-main">
          <span className="row-title">Limit time each day</span>
          <br />
          <span className="row-sub">A total amount of screen time per day.</span>
        </div>
        <Toggle
          checked={dailyOn}
          label="Limit time each day"
          onChange={(on) =>
            onChange({ ...budget, dailyMinutes: on ? 90 : null })
          }
        />
      </div>
      {dailyOn && (
        <div style={{ paddingLeft: 2 }}>
          <Stepper
            label="Daily time limit"
            value={budget.dailyMinutes ?? 90}
            min={15}
            max={1440}
            step={15}
            onChange={(v) => onChange({ ...budget, dailyMinutes: v })}
          />
        </div>
      )}

      {/* Weekly limit */}
      <div className="row-between" style={{ borderTop: "1px solid var(--line)", paddingTop: 14 }}>
        <div className="row-main">
          <span className="row-title">Also set a weekly limit</span>
          <br />
          <span className="row-sub">An overall cap across the whole week.</span>
        </div>
        <Toggle
          checked={weeklyOn}
          label="Also set a weekly limit"
          onChange={(on) =>
            onChange({ ...budget, weeklyMinutes: on ? 600 : null })
          }
        />
      </div>
      {weeklyOn && (
        <div className="stack" style={{ paddingLeft: 2 }}>
          <Stepper
            label="Weekly time limit"
            value={budget.weeklyMinutes ?? 600}
            min={60}
            max={10080}
            step={30}
            onChange={(v) => onChange({ ...budget, weeklyMinutes: v })}
          />
          <div className="field" style={{ margin: 0 }}>
            <span className="field-label">Weeks start on</span>
            <div style={{ display: "flex", gap: 8 }}>
              {(["sun", "mon"] as const).map((ws) => {
                const on = (budget.weekStart ?? "sun") === ws;
                return (
                  <button
                    key={ws}
                    type="button"
                    aria-pressed={on}
                    onClick={() => onChange({ ...budget, weekStart: ws })}
                    style={{
                      flex: 1,
                      minHeight: 44,
                      borderRadius: "var(--radius-control)",
                      border: "1px solid var(--line)",
                      fontWeight: 600,
                      cursor: "pointer",
                      background: on ? "var(--brand-bg)" : "var(--surface)",
                      color: on ? "var(--brand-press)" : "var(--text)",
                    }}
                  >
                    {ws === "sun" ? "Sunday" : "Monday"}
                  </button>
                );
              })}
            </div>
          </div>
        </div>
      )}

      {/* Pause */}
      <div className="row-between" style={{ borderTop: "1px solid var(--line)", paddingTop: 14 }}>
        <div className="row-main">
          <span className="row-title">Pause the daily limit</span>
          <br />
          <span className="row-sub">Lift the time cap for now. The schedule still applies.</span>
        </div>
        <Toggle
          checked={budget.paused ?? false}
          label="Pause the daily limit"
          onChange={(on) => onChange({ ...budget, paused: on })}
        />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Add-an-app affordance (demonstrates the future per-app model)
// ---------------------------------------------------------------------------

const APP_SUGGESTIONS: { appId: string; label: string }[] = [
  { appId: "app_examplegame", label: "ExampleGame" },
  { appId: "app_roblox", label: "Roblox" },
  { appId: "app_youtube", label: "YouTube" },
  { appId: "app_browser", label: "Web browser" },
];

function slugify(label: string): string {
  return (
    "app_" +
    (label
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "_")
      .replace(/^_|_$/g, "") || "custom")
  );
}

function AddAppLimit({ onAdd }: { onAdd: (app: { appId: string; label: string }) => void }) {
  const [open, setOpen] = useState(false);
  const [custom, setCustom] = useState("");

  if (!open) {
    return (
      <Button variant="secondary" block onClick={() => setOpen(true)}>
        + Add a game or app
      </Button>
    );
  }

  return (
    <Card tinted>
      <h3 className="card-title">Add a game or app</h3>
      <p className="card-sub" style={{ marginBottom: 12 }}>
        Pick a game or app to block it from opening, or limit it to set hours.
        Kintrinsic enforces this on the device and never reports what was played.
      </p>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
        {APP_SUGGESTIONS.map((app) => (
          <button
            key={app.appId}
            type="button"
            className="pill pill-neutral pill-action"
            onClick={() => {
              onAdd(app);
              setOpen(false);
            }}
          >
            + {app.label}
          </button>
        ))}
      </div>
      <div className="field" style={{ margin: "14px 0 0" }}>
        <label className="field-label" htmlFor="custom-app">
          Or name another app
        </label>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            id="custom-app"
            className="input"
            placeholder="e.g. Drawing app"
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
          />
          <Button
            variant="primary"
            disabled={custom.trim().length === 0}
            onClick={() => {
              const label = custom.trim();
              if (!label) return;
              onAdd({ appId: slugify(label), label });
              setCustom("");
              setOpen(false);
            }}
          >
            Add
          </Button>
        </div>
      </div>
      <Button
        variant="ghost"
        block
        onClick={() => {
          setOpen(false);
          setCustom("");
        }}
      >
        Cancel
      </Button>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Device (whole computer) editor — the main card
// ---------------------------------------------------------------------------

/** A small segmented (single-choice) control, inline-styled. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            onClick={() => onChange(o.value)}
            style={{
              padding: "8px 12px",
              borderRadius: "var(--radius-control)",
              border: `1px solid ${on ? "var(--brand)" : "var(--hairline, #2a3350)"}`,
              background: on ? "var(--brand-bg)" : "transparent",
              color: on ? "var(--brand)" : "var(--text)",
              fontWeight: on ? 700 : 500,
              cursor: "pointer",
            }}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

/**
 * What happens to a story that's already playing when the lock lands.
 *
 * Framed around the child's experience rather than the mechanism, because the
 * decision is about whether an audiobook gets cut off mid-sentence — not about
 * package suspension.
 */
function ListeningEditor({
  listening,
  sections,
  onChange,
  wardName,
}: {
  listening: ListeningPolicy;
  /** The ward's reported inventory, grouped one section per paired device —
   *  see `AppsEditor`'s identical prop. Listening is never split per device,
   *  so this is always the full grouped picker. */
  sections: AppSection[];
  onChange: (l: ListeningPolicy) => void;
  /** For the "from <ward>'s account" mark on a user-installed row. */
  wardName: string;
}) {
  const MODES: { id: ListeningPolicy["mode"]; title: string; sub: string }[] = [
    { id: "stop", title: "Stop with the screen", sub: "Time's up means everything, audio included." },
    { id: "continue", title: "Let it keep playing", sub: "The screen shades; the story carries on." },
    { id: "grace", title: "Let it finish, then stop", sub: "A set amount of listening after the lock." },
  ];
  return (
    <div className="stack">
      {MODES.map((m) => (
        <label key={m.id} className="row-between" style={{ gap: 10, alignItems: "flex-start" }}>
          <span className="row-main">
            <span className="row-title">{m.title}</span>
            <br />
            <span className="row-sub">{m.sub}</span>
          </span>
          <input
            type="radio"
            name="listening-mode"
            checked={listening.mode === m.id}
            onChange={() => onChange({ ...listening, mode: m.id })}
          />
        </label>
      ))}

      {listening.mode === "grace" && (
        <Stepper
          label="Listening after the lock"
          value={listening.graceMinutes ?? 30}
          min={5}
          max={240}
          step={5}
          onChange={(v) => onChange({ ...listening, graceMinutes: v })}
        />
      )}

      {listening.mode !== "stop" && (
        <div style={{ borderTop: "1px solid var(--line)", paddingTop: 12 }}>
          <span className="row-sub">
            Which apps count as listening? Only these may carry on, and only
            while they're actually playing something.
          </span>
          {sections.length === 0 ? (
            <p className="card-sub" style={{ marginTop: 8 }}>
              Nothing to choose yet — the list fills in once their device has
              checked in.
            </p>
          ) : (
            // Grouped by device — same idiom as the Apps section and Named
            // times pickers: a merged list mixes a laptop's flatpak ids in
            // with a phone's Android packages as though they were one
            // vocabulary.
            sections.map((s) => (
              <div key={s.deviceId} style={{ marginTop: 10 }}>
                <div className="row-sub" style={{ marginBottom: 4 }}>
                  {s.platform === "android" ? "📱" : "💻"} {s.label}
                </div>
                {s.apps.length === 0 ? (
                  <p className="card-sub" style={{ margin: 0 }}>
                    Nothing to choose yet — the list fills in once this device
                    has checked in.
                  </p>
                ) : (
                  <div className="stack">
                    {s.apps.map((a) => {
                      const on = listening.apps.includes(a.pkg);
                      return (
                        <label key={a.pkg} className="row-between" style={{ gap: 10 }}>
                          <span className="row-title">
                            {a.label}
                            {a.userInstalled && (
                              <span className="muted" style={{ fontSize: "var(--fs-small)", marginLeft: 8 }}>
                                from {wardName}'s account
                              </span>
                            )}
                          </span>
                          <input
                            type="checkbox"
                            checked={on}
                            onChange={(e) =>
                              onChange({
                                ...listening,
                                apps: e.target.checked
                                  ? [...listening.apps, a.pkg]
                                  : listening.apps.filter((p) => p !== a.pkg),
                              })
                            }
                          />
                        </label>
                      );
                    })}
                  </div>
                )}
              </div>
            ))
          )}
          {listening.apps.length === 0 && (
            <div style={{ marginTop: 10 }}>
              <Banner tone="info">
                Nothing is named yet, so audio still stops with the screen.
              </Banner>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Always available: apps open at any hour, standing or time-boxed
// ---------------------------------------------------------------------------

const WEEKDAY_SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTH_SHORT = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/** "Sun 3 Aug, 9:00am" — a preset never hides which moment it actually signs.
 *  Resolved against the guardian's own clock, like every other absolute-instant
 *  picker on this screen (`untilBedtime`, the app-hold sheet). */
function formatUntilInstant(untilUnix: number): string {
  const d = new Date(untilUnix * 1000);
  const weekday = WEEKDAY_SHORT[d.getDay()];
  const month = MONTH_SHORT[d.getMonth()];
  let h = d.getHours();
  const ampm = h >= 12 ? "pm" : "am";
  h = h % 12 || 12;
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${weekday} ${d.getDate()} ${month}, ${h}:${mm}${ampm}`;
}

/** The next occurrence of `weekday` (0=Sun) at `hour:minute`, strictly in the
 *  future — so pressing the preset on the target day itself after that time
 *  rolls to next week rather than silently picking a moment already past. */
function nextWeekdayAt(now: Date, weekday: number, hour: number, minute: number): Date {
  const d = new Date(now);
  const diff = (weekday - d.getDay() + 7) % 7;
  d.setDate(d.getDate() + diff);
  d.setHours(hour, minute, 0, 0);
  if (d.getTime() <= now.getTime()) d.setDate(d.getDate() + 7);
  return d;
}

interface AlwaysAvailablePreset {
  label: string;
  untilUnix: number;
}

/**
 * Night-spanning presets, unlike `HOLD_PRESETS` (same-day: 15 min .. 2 hours).
 * A sleepover is two nights out, not an hour — "tomorrow morning" and "Sunday
 * morning" both anchor to 9am so the ward isn't shut out at the start of a day,
 * and "a week" covers a longer stay without asking the guardian to do the
 * arithmetic themselves. Computed from the GUARDIAN's local clock, matching
 * every other absolute-instant picker on this screen.
 */
function alwaysAvailablePresets(nowUnix: number): AlwaysAvailablePreset[] {
  const now = new Date(nowUnix * 1000);
  const tomorrow = new Date(now);
  tomorrow.setDate(tomorrow.getDate() + 1);
  tomorrow.setHours(9, 0, 0, 0);
  const sunday = nextWeekdayAt(now, 0, 9, 0);
  const week = new Date(now.getTime() + 7 * 24 * 60 * 60 * 1000);
  return [
    { label: "Tomorrow morning", untilUnix: Math.floor(tomorrow.getTime() / 1000) },
    { label: "Sunday morning", untilUnix: Math.floor(sunday.getTime() / 1000) },
    { label: "A week", untilUnix: Math.floor(week.getTime() / 1000) },
  ];
}

/**
 * The absolute-instant picker for one always-available app's expiry — mirrors
 * `AppHoldSheet`'s shape and `onPick: (untilUnix: number) => void` contract so
 * the two read as one idiom to a guardian who has already used a hold. Its
 * presets do NOT fit here, though: `AppHoldSheet` is built for "allow this for
 * an hour", same-day spans, where this sheet exists for a sleepover — two
 * nights out — so every preset is its own night-spanning one (Step 5).
 */
function AlwaysAvailableExpirySheet({
  label,
  onPick,
  onCancel,
}: {
  /** The app's own name — a package id means nothing at a glance. */
  label: string;
  onPick: (untilUnix: number) => void;
  onCancel: () => void;
}) {
  const presets = alwaysAvailablePresets(Math.floor(Date.now() / 1000));
  return (
    <div
      className="sheet-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={`${label} — until when?`}
      onClick={onCancel}
    >
      <div className="sheet" onClick={(e) => e.stopPropagation()}>
        <h2 className="card-title">{label} — until when?</h2>
        <p className="card-sub" style={{ marginBottom: 14 }}>
          It goes back to blocked on its own — nothing for you to remember.
        </p>
        <div
          role="group"
          aria-label="Until when"
          style={{ display: "flex", flexWrap: "wrap", gap: 8, marginBottom: 10 }}
        >
          {presets.map((p) => (
            <Button
              key={p.label}
              variant="secondary"
              onClick={() => onPick(p.untilUnix)}
              style={{ flex: "1 1 100%", textAlign: "left" }}
            >
              {p.label}
              <br />
              <span className="row-sub">until {formatUntilInstant(p.untilUnix)}</span>
            </Button>
          ))}
        </div>
        <Button variant="secondary" block onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

function AlwaysAvailableEditor({
  alwaysAvailable,
  sections,
  onChange,
  wardName,
}: {
  alwaysAvailable: AlwaysAvailablePolicy;
  /** The ward's reported inventory, grouped one section per paired device —
   *  see `ListeningEditor`'s identical prop. Never split per device: an app
   *  either stays open everywhere it's named, or it isn't named. */
  sections: AppSection[];
  onChange: (a: AlwaysAvailablePolicy) => void;
  /** For the "from <ward>'s account" mark on a user-installed row. */
  wardName: string;
}) {
  // Which app's expiry sheet is open, by package. Null = none.
  const [expiryFor, setExpiryFor] = useState<string | null>(null);
  const entryFor = (pkg: string) => alwaysAvailable.apps.find((e) => e.pkg === pkg);
  const setEntry = (pkg: string, untilUnix: number | undefined) => {
    const rest = alwaysAvailable.apps.filter((e) => e.pkg !== pkg);
    onChange({
      apps: untilUnix === undefined ? [...rest, { pkg }] : [...rest, { pkg, untilUnix }],
    });
  };
  const removeEntry = (pkg: string) => {
    onChange({ apps: alwaysAvailable.apps.filter((e) => e.pkg !== pkg) });
  };
  const expiryLabel =
    sections.flatMap((s) => s.apps).find((a) => a.pkg === expiryFor)?.label ?? expiryFor ?? "";

  return (
    <div className="stack">
      <p className="card-sub">
        These apps open at any hour, even when the phone is otherwise locked
        or set to stay off until you open it, and their time is never counted
        against their limit. An audiobook player at 2am, a messaging app at a
        sleepover.
      </p>
      <p className="card-sub">
        They stay shut for a &ldquo;Finish now&rdquo;, and an app you have
        blocked in Apps stays blocked. Kintrinsic cannot filter inside an app it
        is letting through &mdash; choose apps you would be content with
        unsupervised.
      </p>

      {sections.length === 0 ? (
        <p className="card-sub" style={{ marginTop: 8 }}>
          Nothing to choose yet — the list fills in once their device has
          checked in.
        </p>
      ) : (
        // Grouped by device — same idiom as the Apps and Listening pickers: a
        // merged list mixes a laptop's flatpak ids in with a phone's Android
        // packages as though they were one vocabulary.
        sections.map((s) => (
          <div key={s.deviceId} style={{ marginTop: 10 }}>
            <div className="row-sub" style={{ marginBottom: 4 }}>
              {s.platform === "android" ? "📱" : "💻"} {s.label}
            </div>
            {s.apps.length === 0 ? (
              <p className="card-sub" style={{ margin: 0 }}>
                Nothing to choose yet — the list fills in once this device
                has checked in.
              </p>
            ) : (
              <div className="stack">
                {s.apps.map((a) => {
                  const entry = entryFor(a.pkg);
                  const on = entry !== undefined;
                  return (
                    <div key={a.pkg} className="stack" style={{ gap: 4 }}>
                      <label className="row-between" style={{ gap: 10 }}>
                        <span className="row-title">
                          {a.label}
                          {a.userInstalled && (
                            <span
                              className="muted"
                              style={{ fontSize: "var(--fs-small)", marginLeft: 8 }}
                            >
                              from {wardName}'s account
                            </span>
                          )}
                        </span>
                        <input
                          type="checkbox"
                          checked={on}
                          onChange={(e) =>
                            e.target.checked ? setEntry(a.pkg, undefined) : removeEntry(a.pkg)
                          }
                        />
                      </label>
                      {on && (
                        <div
                          className="row-between"
                          style={{ gap: 10, paddingLeft: 4, flexWrap: "wrap" }}
                        >
                          <span style={{ display: "flex", gap: 14 }}>
                            <label style={{ display: "flex", alignItems: "center", gap: 4 }}>
                              <input
                                type="radio"
                                name={`always-available-${a.pkg}`}
                                checked={entry?.untilUnix === undefined}
                                onChange={() => setEntry(a.pkg, undefined)}
                              />
                              Always
                            </label>
                            <label style={{ display: "flex", alignItems: "center", gap: 4 }}>
                              <input
                                type="radio"
                                name={`always-available-${a.pkg}`}
                                checked={entry?.untilUnix !== undefined}
                                onChange={() => setExpiryFor(a.pkg)}
                              />
                              Until…
                            </label>
                          </span>
                          {entry?.untilUnix !== undefined && (
                            <span className="row-sub">
                              until {formatUntilInstant(entry.untilUnix)}
                            </span>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        ))
      )}
      {alwaysAvailable.apps.length === 0 && (
        <div style={{ marginTop: 10 }}>
          <Banner tone="info">Nothing is named yet, so the lock takes everything.</Banner>
        </div>
      )}

      {expiryFor && (
        <AlwaysAvailableExpirySheet
          label={expiryLabel}
          onPick={(untilUnix) => {
            setEntry(expiryFor, untilUnix);
            setExpiryFor(null);
          }}
          onCancel={() => setExpiryFor(null)}
        />
      )}
    </div>
  );
}

/** `normalizeWebDomain` in the shape `DomainList`'s `parse` takes. */
function parseWebDomain(s: string): { ok: true; value: string } | { ok: false; message: string } {
  const r = normalizeWebDomain(s);
  return r.ok ? { ok: true, value: r.domain } : r;
}

/** Editable list of site domains — add via input, remove via chip. */
function DomainList({
  label,
  hint,
  items,
  placeholder,
  onChange,
  normalize = (s) => s.trim().toLowerCase(),
  reject,
  rejectMessage,
  parse,
}: {
  label: string;
  hint: string;
  items: string[];
  placeholder: string;
  onChange: (next: string[]) => void;
  /** How to canonicalize an entry on add (domains lowercase; package names as-is). */
  normalize?: (s: string) => string;
  /** An entry this list must never accept as free text (SAFETY: a
   *  `cmdline:` identity may only ever enter a saved policy through the
   *  curated launch-signatures table — see `domain/launchSignatures.ts`). */
  reject?: (v: string) => boolean;
  rejectMessage?: string;
  /** Canonicalize AND vet an entry, with its own message on refusal — for
   *  lists (web domains) where a malformed entry signs fine and then silently
   *  enforces nothing. Takes the place of `normalize` when given. */
  parse?: (s: string) => { ok: true; value: string } | { ok: false; message: string };
}) {
  const [input, setInput] = useState("");
  const [error, setError] = useState<string | null>(null);
  const add = () => {
    let v: string;
    if (parse) {
      if (!input.trim()) return;
      const r = parse(input);
      if (!r.ok) {
        setError(r.message);
        return;
      }
      v = r.value;
    } else {
      v = normalize(input);
    }
    if (!v) return;
    if (reject?.(v)) {
      setError(rejectMessage ?? "That identifier isn't allowed here.");
      return;
    }
    if (!items.includes(v)) onChange([...items, v]);
    setInput("");
    setError(null);
  };
  return (
    <div>
      <div className="field-label">{label}</div>
      <p className="card-sub" style={{ margin: "2px 0 8px" }}>{hint}</p>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginBottom: 8 }}>
        {items.length === 0 && <span className="muted">None yet</span>}
        {items.map((d) => (
          <span
            key={d}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 6,
              padding: "4px 8px",
              borderRadius: 999,
              background: "var(--brand-bg)",
              fontSize: "var(--fs-small)",
            }}
          >
            {d}
            <button
              type="button"
              aria-label={`Remove ${d}`}
              onClick={() => onChange(items.filter((x) => x !== d))}
              style={{ border: "none", background: "none", cursor: "pointer", color: "var(--muted)", fontSize: 16, lineHeight: 1 }}
            >
              ×
            </button>
          </span>
        ))}
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <input
          value={input}
          onChange={(e) => {
            setInput(e.target.value);
            if (error) setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          placeholder={placeholder}
          style={{
            flex: 1,
            padding: "8px 12px",
            borderRadius: "var(--radius-control)",
            border: "1px solid var(--hairline, #2a3350)",
            background: "var(--surface, #141b36)",
            color: "var(--text)",
          }}
        />
        <Button variant="secondary" onClick={add}>Add</Button>
      </div>
      {error && (
        <p className="card-sub" style={{ color: "var(--warn, #9a6516)", marginTop: 4 }}>
          {error}
        </p>
      )}
    </div>
  );
}

/** The parent-authoritative web-content controls (v1). */
function WebEditor({ web, onChange }: { web: WebPolicy; onChange: (w: WebPolicy) => void }) {
  const set = (patch: Partial<WebPolicy>) => onChange({ ...web, ...patch });
  return (
    <div className="stack">
      <div className="row-between">
        <span className="row-sub">Filter websites</span>
        <Toggle
          checked={web.enabled}
          label="Filter websites"
          onChange={(on) => set({ enabled: on })}
        />
      </div>
      {!web.enabled ? (
        <p className="card-sub" style={{ marginTop: 0 }}>
          Web filtering is off — every site is allowed. Turn it on to allow or
          block specific sites. (Enforced on computers and phones.)
        </p>
      ) : (
        <div className="stack">
          <div>
            <div className="field-label">Mode</div>
            <Segmented
              value={web.posture}
              onChange={(v) => set({ posture: v })}
              options={[
                { value: "blocklist", label: "Block listed sites" },
                { value: "allowlist", label: "Only allow listed" },
              ]}
            />
          </div>
          <DomainList
            label={web.posture === "allowlist" ? "Allowed sites" : "Always allow"}
            hint={
              web.posture === "allowlist"
                ? "Only these load (plus safe curated ones). Everything else is blocked."
                : "Exceptions that always load, even if otherwise blocked."
            }
            items={web.allow}
            placeholder="wikipedia.org"
            parse={parseWebDomain}
            onChange={(allow) => set({ allow })}
          />
          <DomainList
            label={web.posture === "allowlist" ? "Also block" : "Blocked sites"}
            hint="These never load — they outrank everything else."
            items={web.block}
            placeholder="youtube.com"
            parse={parseWebDomain}
            onChange={(block) => set({ block })}
          />
          <div>
            <div className="field-label">YouTube</div>
            <Segmented
              value={web.youtube}
              onChange={(v) => set({ youtube: v })}
              options={[
                { value: "off", label: "Off" },
                { value: "moderate", label: "Moderate" },
                { value: "strict", label: "Strict" },
              ]}
            />
          </div>
          <div className="row-between">
            <span className="row-sub">SafeSearch on Google, Bing &amp; DuckDuckGo</span>
            <Toggle
              checked={web.safeSearch}
              label="SafeSearch"
              onChange={(on) => set({ safeSearch: on })}
            />
          </div>
          <div>
            <div className="field-label">Age preset</div>
            <Segmented
              value={web.ageTier}
              onChange={(v) => set({ ageTier: v })}
              options={[
                { value: "older", label: "Older" },
                { value: "young", label: "Young" },
              ]}
            />
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Learning time: apps whose focused time is time-free (it never drains the
 * screen-time budget — homework must not cost Minecraft time). Site apps come
 * from the curated catalogue (verified domain pins); native apps are picked
 * from what the computer reports; a home-built project can be vouched for by
 * path (that identity is your word, not something the computer can verify).
 */
/** Common durations for a time-boxed hotspot grant (minutes). */

/**
 * The communication lifeline editor (spec D9): up to three guardian numbers
 * the ward can ALWAYS call from the lock screen — even mid-lockout. A locked
 * phone is still a phone.
 */
/**
 * "This control does nothing on that device." Rendered under a section whose
 * clause one of the ward's platforms never reads (see `domain/wardenSupport`).
 *
 * Deliberately a quiet note rather than a warning banner: nothing is wrong, and
 * the guardian has done nothing to correct. It is the standing shape of the
 * family's devices, so it should read as a fact about the world — the loud
 * treatment is reserved for the too-old case, which the parent CAN fix by
 * updating. Renders nothing when every device can honour the control.
 */
function ParityNote({ note }: { note: string | null }) {
  if (!note) return null;
  return (
    <p className="card-sub" style={{ marginTop: 10 }}>
      {note}
    </p>
  );
}

function LifelineEditor({
  lifeline,
  onChange,
  v2Ready,
}: {
  lifeline: Lifeline;
  onChange: (l: Lifeline) => void;
  /** Every paired phone accepts the v2 lifeline (5 numbers, emergency entry,
   *  break-glass). Until then the editor stays on the v1 shape — sending v2
   *  to an older phone would leave the ward with NO lifeline at all. */
  v2Ready: boolean;
}) {
  const rows = lifeline.numbers;
  const maxRows = v2Ready ? MAX_LIFELINE_NUMBERS : 3;
  const bg = lifeline.breakGlass ?? { enabled: false, scope: "full" as const, durationMinutes: 10 };
  const set = (i: number, patch: Partial<{ label: string; number: string }>) => {
    const next = rows.map((r, j) => (j === i ? { ...r, ...patch } : r));
    onChange({ ...lifeline, numbers: next });
  };
  return (
    <div className="stack">
      <span className="row-sub">
        Numbers your ward can always call from the lock screen — even when
        their time is up.
      </span>
      {rows.map((r, i) => (
        <div key={i} className="row-between" style={{ gap: 8 }}>
          <input
            className="input"
            style={{ flex: 1 }}
            placeholder="Name (e.g. Mum)"
            maxLength={20}
            value={r.label}
            onChange={(e) => set(i, { label: e.target.value })}
          />
          <input
            className="input"
            style={{ flex: 1.4 }}
            placeholder="Phone number"
            inputMode="tel"
            value={r.number}
            onChange={(e) => set(i, { number: e.target.value })}
          />
          <Button
            variant="ghost"
            aria-label={`Remove ${r.label || "number"}`}
            onClick={() =>
              onChange({ ...lifeline, numbers: rows.filter((_, j) => j !== i) })
            }
          >
            ✕
          </Button>
        </div>
      ))}
      {rows.length < maxRows && (
        <Button
          variant="ghost"
          onClick={() =>
            onChange({ ...lifeline, numbers: [...rows, { label: "", number: "" }] })
          }
        >
          + Add a number
        </Button>
      )}

      {v2Ready ? (
        <>
          <label className="row-between" style={{ gap: 8, marginTop: 12 }}>
            <span>
              Show the emergency number
              <span className="row-sub" style={{ display: "block" }}>
                The phone fills in its own country's number (999, 911, 112…),
                so it's right at home and abroad.
              </span>
            </span>
            <input
              type="checkbox"
              checked={lifeline.emergencyServices ?? false}
              onChange={(e) => onChange({ ...lifeline, emergencyServices: e.target.checked })}
            />
          </label>

          <label className="row-between" style={{ gap: 8, marginTop: 12 }}>
            <span>
              Torch
              <span className="row-sub" style={{ display: "block" }}>
                Puts a torch on the lock screen. A locked phone is still a
                light — walking home in the dark shouldn't mean using the
                emergency unlock just to see. Hidden on a phone with no flash.
              </span>
            </span>
            <input
              type="checkbox"
              checked={lifeline.torch ?? false}
              onChange={(e) => onChange({ ...lifeline, torch: e.target.checked })}
            />
          </label>

          <div className="stack" style={{ marginTop: 12 }}>
            <label className="row-between" style={{ gap: 8 }}>
              <span>
                Emergency unlock
                <span className="row-sub" style={{ display: "block" }}>
                  Your ward can always open their phone in an emergency —
                  no waiting for you. You're told straight away, and it shows
                  in their week. There's no limit on using it: if it's being
                  used a lot, that's a conversation, not a lockout.
                </span>
              </span>
              <input
                type="checkbox"
                checked={bg.enabled}
                onChange={(e) =>
                  onChange({ ...lifeline, breakGlass: { ...bg, enabled: e.target.checked } })
                }
              />
            </label>
            {/* Turning this OFF is the one setting here that can strand a
                device, so it says so plainly rather than letting a guardian
                find out from a tablet they can no longer open. */}
            {!bg.enabled && (
              <Banner tone="warn">
                With this off, a device that can’t reach the internet can’t be
                opened at all — not by you, not by them. If it moves somewhere
                with different Wi-Fi while it’s locked, it can’t collect the time
                you give it and there’s no way in from the lock screen. Getting
                it back then means a USB cable, or wiping it and starting again.
              </Banner>
            )}
            {bg.enabled && (
              <div className="row-between" style={{ gap: 8 }}>
                <select
                  className="input"
                  value={bg.scope}
                  onChange={(e) =>
                    onChange({
                      ...lifeline,
                      breakGlass: { ...bg, scope: e.target.value as "calls" | "full" },
                    })
                  }
                >
                  <option value="full">Opens the whole phone</option>
                  {/* "Opens calls only" overstated it: the device lifts the
                      lock for `full` alone, and the lifeline numbers already
                      dial from the shade whether or not the glass is broken.
                      What this setting really does is let them raise the alarm
                      with you — so it says that. */}
                  <option value="calls">Tells you — the phone stays locked</option>
                </select>
                <select
                  className="input"
                  value={bg.durationMinutes}
                  onChange={(e) =>
                    onChange({
                      ...lifeline,
                      breakGlass: { ...bg, durationMinutes: Number(e.target.value) },
                    })
                  }
                >
                  {[5, 10, 15, 30, 60].map((m) => (
                    <option key={m} value={m}>
                      for {m} minutes
                    </option>
                  ))}
                </select>
              </div>
            )}
          </div>
        </>
      ) : (
        <span className="row-sub" style={{ marginTop: 8 }}>
          More numbers, the emergency number and the emergency unlock appear
          once every paired phone has updated Kintrinsic.
        </span>
      )}
    </div>
  );
}

/**
 * The hotspot posture editor. Default OFF; a guardian can share the connection
 * as a *filtered* Kintrinsic hotspot (guests get the same web filter as the ward's
 * own device) or as a plain hotspot (honest: guests are UNFILTERED), optionally
 * for a set window. No competing product filters shared connections at all.
 */
function TetheringEditor({
  tethering,
  onChange,
}: {
  tethering: Tethering;
  onChange: (t: Tethering) => void;
}) {
  const on = tethering.allow !== "none";

  const setWindow = (minutes: number | null) => {
    if (minutes === -1) {
      // "Until I turn it off" — no expiry.
      onChange({ allow: tethering.allow });
    } else {
      const until = minutes == null ? endOfTodayUnix() : Math.floor(Date.now() / 1000) + minutes * 60;
      onChange({ ...tethering, until });
    }
  };

  return (
    <div className="stack">
      <div className="row-between">
        <span className="row-sub">Let this device share its connection</span>
        <Toggle
          checked={on}
          label="Allow hotspot"
          onChange={(o) => onChange(o ? { allow: "filtered" } : { allow: "none" })}
        />
      </div>

      {on && (
        <>
          <SectionLabel>How it’s shared</SectionLabel>
          <Segmented<Tethering["allow"]>
            value={tethering.allow}
            options={[
              { value: "filtered", label: "Filtered (recommended)" },
              { value: "raw", label: "Normal" },
            ]}
            onChange={(v) => onChange({ ...tethering, allow: v })}
          />
          <Banner tone={tethering.allow === "filtered" ? "info" : "warn"}>
            {tethering.allow === "filtered"
              ? "Devices that connect share this device’s web filter — the same rules you set here. They turn the hotspot on at their end when they need it, and it switches itself off once nobody’s using it, so it isn’t sitting on their battery all day. A guest has to point its Wi-Fi at the hotspot’s proxy by hand; apps that ignore a proxy won’t connect. This device stays protected either way."
              : "Devices that connect get UNFILTERED internet. They turn the hotspot on in their own settings when they need it. This device itself stays protected, but anything joining its hotspot does not."}
          </Banner>

          <SectionLabel>For how long</SectionLabel>
          <Segmented<string>
            value={tetherWindowLabel(tethering, Math.floor(Date.now() / 1000))}
            options={TETHER_WINDOWS.map((w) => ({ value: w.label, label: w.label }))}
            onChange={(label) => {
              const w = TETHER_WINDOWS.find((x) => x.label === label);
              if (w) setWindow(w.minutes);
            }}
          />
        </>
      )}
    </div>
  );
}

/**
 * Add an educational site the curated catalogue doesn't carry.
 *
 * The catalogue's closures are measured against the live site; one typed here
 * cannot be, so it is pinned to the address itself and its subdomains (see
 * `domain/siteClosure`). That errs narrow — a site whose pictures come from
 * somewhere else renders without them — and the copy says so, because a child
 * staring at a broken page deserves an explanation their parent can give.
 */
export function AddSiteForm({
  takenIds,
  onAdd,
}: {
  takenIds: string[];
  onAdd: (app: LearningAppSel) => void;
}) {
  const [label, setLabel] = useState("");
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);

  const submit = () => {
    const name = label.trim();
    if (!name) {
      setError("Give the site a name so they know what it is.");
      return;
    }
    const closure = siteClosureFor(url);
    if (!closure.ok) {
      setError(siteClosureMessage(closure.error));
      return;
    }
    onAdd({
      // The catalogue's ids are taken too: a guardian's own "Wikipedia"
      // (their URL, their domains) must never mint the catalogue's id, or
      // `learningAppPool` would swap the catalogue's entry in over it.
      id: siteIdFor(name, [...takenIds, ...LEARNING_CATALOGUE.map((c) => c.id)]),
      label: name,
      kind: "site",
      url: closure.url,
      domains: closure.domains,
    });
    setLabel("");
    setUrl("");
    setError(null);
  };

  return (
    <div className="stack" style={{ gap: 6, marginTop: 8 }}>
      <div className="field-label">Add another site</div>
      <input
        className="input"
        placeholder="Name (e.g. Maths Genie)"
        aria-label="Site name"
        value={label}
        onChange={(e) => setLabel(e.target.value)}
      />
      <input
        className="input"
        placeholder="www.example.org"
        aria-label="Site address"
        value={url}
        onChange={(e) => setUrl(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && submit()}
      />
      {error && (
        <p className="card-sub" style={{ margin: 0, color: "var(--danger)" }}>
          {error}
        </p>
      )}
      <p className="card-sub" style={{ margin: 0 }}>
        Sites you add are locked to their own address. Some sites load their
        pictures or videos from somewhere else and may look broken — tell us
        which and we’ll set them up properly.
      </p>
      <Button variant="secondary" onClick={submit}>
        Add site
      </Button>
    </div>
  );
}

/**
 * Everything the Apps list needs to put an app on hold. Bundled as one prop
 * because it is one feature, and because `Split` hands this editor a
 * device-routed `onChange` that the hold must go through unchanged — that is
 * what makes a hold on a split control land on the right device without any
 * split-aware code being written twice.
 */
interface HoldCtx {
  /** A clock that ticks only while something is counting down. */
  nowUnix: number;
  /** The ward's schedule, for "Until bedtime". */
  schedule?: Schedule;
  /** Paired devices reporting a Kintrinsic too old to honour a hold, named. */
  tooOldNote: string | null;
  /** Something else on this screen is unsaved and will travel with the hold. */
  otherChangesPending: boolean;
  /** Publish now: the draft (hold included) goes through the ordinary save. */
  requestSave: () => void;
  saving: boolean;
}

/**
 * Everything the "Remove from device" subsection needs. Its own bundle rather
 * than three loose props for the same reason `HoldCtx` is: it is one feature,
 * and every one of these is about whether the ward's devices can honour it —
 * removal is Android Device Owner only (`wardenSupport`'s `appHide`), so a
 * guardian must be told before they press it, not after nothing happens.
 */
interface HideCtx {
  /** Could ANY paired device actually hide an app right now? False greys the
   *  Remove buttons — putting an app BACK stays available regardless, since
   *  that is the direction that can only ever help. */
  canRemove: boolean;
  /** Paired devices reporting a Kintrinsic too old to hide an app, named. */
  tooOldNote: string | null;
  /** Devices whose platform never hides apps at any version (a laptop), named. */
  parityNote: string | null;
}

function AppsEditor({
  apps,
  available,
  sections,
  onChange,
  hold,
  hide,
  coversComputer = false,
  wardName,
}: {
  apps: AppsPolicy;
  /** Apps the device reported (pkg + label, + userInstalled, + hidden) — the
   *  picker source. `hidden` is what the device says it is CURRENTLY hiding,
   *  which is not always what this draft asks for (see `removed` below). */
  available: { pkg: string; label: string; userInstalled?: boolean; hidden?: boolean }[];
  /** The SAME inventory as `available`, grouped one section per paired
   *  device. Present only when this editor covers the whole ward (the
   *  unsplit render) — a per-device copy already IS one device's list,
   *  headed by that device's own name above it, so passing sections there
   *  would just repeat the heading. Undefined falls back to the flat list
   *  this editor always showed. */
  sections?: AppSection[];
  onChange: (a: AppsPolicy) => void;
  /** Absent = no holds offered (the app-scope card reuses this editor). */
  hold?: HoldCtx;
  /** Absent = no "Remove from device" affordance at all — a card with no
   *  device inventory behind it has nothing to remove. */
  hide?: HideCtx;
  /** Does this rule reach a computer? Drives the allowlist caveat below. */
  coversComputer?: boolean;
  /** The ward's own name, for the "from <ward>'s account" mark on a
   *  user-installed row. Absent only for the app-scope card, which has no
   *  device inventory (and so no user-installed entries) to mark. */
  wardName?: string;
}) {
  const set = (patch: Partial<AppsPolicy>) => onChange({ ...apps, ...patch });
  const isAllow = apps.posture === "allowlist";
  const list = isAllow ? apps.allowed : apps.blocked;
  const setList = (next: string[]) => set(isAllow ? { allowed: next } : { blocked: next });
  const toggle = (pkg: string) => {
    const next: AppsPolicy = {
      ...apps,
      ...(isAllow
        ? { allowed: list.includes(pkg) ? list.filter((p) => p !== pkg) : [...list, pkg] }
        : { blocked: list.includes(pkg) ? list.filter((p) => p !== pkg) : [...list, pkg] }),
    };
    // Changing the STANDING rule for an app supersedes the temporary one.
    // Leaving the hold behind would mean flipping the switch appeared to do
    // nothing at all, because the hold would still be what the phone enforced.
    onChange(hold ? putHold(next, pkg, "allowed", null, hold.nowUnix) : next);
  };
  const reportedPkgs = new Set(available.map((a) => a.pkg));
  // Manually-typed entries not in the reported inventory (still editable).
  const extras = list.filter((p) => !reportedPkgs.has(p));
  const verb = isAllow ? "allow" : "block";
  // Which app's hold sheet is open, by package. Null = none.
  const [holdFor_, setHoldFor] = useState<string | null>(null);
  const openApp = available.find((a) => a.pkg === holdFor_);

  // --- "Remove from device" ------------------------------------------------
  // Deliberately outside everything above: hiding an app is not a rule about
  // the child's day, so it is neither a posture nor gated on the "Control
  // apps" switch (the wire agrees — `appsToGrant` emits `hidden` before the
  // paused early-return).
  const hiddenList = apps.hidden ?? [];
  const hiddenSet = new Set(hiddenList);
  const setHidden = (next: string[]) => {
    const out: AppsPolicy = { ...apps, hidden: next };
    // Absent when empty, never `[]` — an empty array would be a change the
    // dirty-check can see and the wire cannot (`buildApps` drops it too), so
    // the Apps row would sit unsaved for ever after the last Put back.
    if (!next.length) delete out.hidden;
    onChange(out);
  };
  // Apps the DEVICE reports as hidden that this draft has no record of —
  // a rule authored before a device split, or an inventory that hasn't caught
  // up with the last save. They are still listed as removed, because that is
  // what the device is actually doing, and the guardian must have something
  // to press "Put back" on. `putBack` remembers a press on one of THOSE: the
  // draft never listed it, so filtering it out is a no-op and the row would
  // otherwise stick under Removed for ever, looking broken. The clause about
  // to be signed already doesn't hide it, so the device restores it anyway.
  const [putBack, setPutBack] = useState<string[]>([]);
  const removed = [
    ...hiddenList,
    ...available
      .filter((a) => a.hidden && !hiddenSet.has(a.pkg) && !putBack.includes(a.pkg))
      .map((a) => a.pkg),
  ];
  const removedSet = new Set(removed);
  const labelForPkg = (pkg: string) => available.find((a) => a.pkg === pkg)?.label ?? pkg;
  const restore = (pkg: string) => {
    setHidden(hiddenList.filter((p) => p !== pkg));
    if (!hiddenSet.has(pkg)) setPutBack((prev) => [...prev, pkg]);
  };

  /** One app's row — same markup whether it's under a flat list or a
   *  per-device section. */
  const renderAppRow = (a: { pkg: string; label: string; userInstalled?: boolean }) => {
    const live = hold ? holdFor(apps, a.pkg, hold.nowUnix) : undefined;
    return (
      <div className="row-between" key={a.pkg} style={{ padding: "4px 0" }}>
        <span>
          {a.label}
          <span className="muted" style={{ fontSize: "var(--fs-small)", marginLeft: 8 }}>
            {a.pkg}
          </span>
          {/* A ward-writable inventory entry — flagged so it's never
              mistaken for something the ward has no way to have altered.
              Says WHERE it lives, never WHO put it there (F3 review:
              contract.md's userInstalled explicitly covers a PARENT running
              `flatpak install --user` FOR their child too, so "installed by"
              claims an agency the flag doesn't carry). */}
          {a.userInstalled && wardName && (
            <span className="muted" style={{ fontSize: "var(--fs-small)", marginLeft: 8 }}>
              from {wardName}'s account
            </span>
          )}
          {/* Level-triggered from the hold's own instant, never a
              latched flag: a countdown that outlived the phone's
              reading of it is the disagreement between the two
              screens that costs a family their trust. */}
          {live && (
            <>
              <br />
              <span
                className="row-sub"
                aria-live="polite"
                style={{ color: "var(--warn, #9a6516)" }}
              >
                {holdRowLabel(live, hold!.nowUnix)}
              </span>
            </>
          )}
        </span>
        <span style={{ display: "flex", alignItems: "center", gap: 10 }}>
          {hold && (
            <button
              type="button"
              // The label follows what the press would DO, so a
              // screen reader never announces the opposite.
              aria-label={
                live
                  ? `Change how long ${a.label} is ${
                      live.state === "allowed" ? "open" : "paused"
                    }`
                  : holdDirection(apps, a.pkg, hold.nowUnix) === "allowed"
                    ? `Allow ${a.label} for a while`
                    : `Pause ${a.label} for a while`
              }
              onClick={() => setHoldFor(a.pkg)}
              className="btn btn-secondary"
              style={{ padding: "4px 10px", minWidth: 0, lineHeight: 1.1 }}
            >
              <span aria-hidden="true">⏱</span>
            </button>
          )}
          <Toggle
            checked={list.includes(a.pkg)}
            label={`${verb} ${a.label}`}
            onChange={() => toggle(a.pkg)}
          />
        </span>
      </div>
    );
  };

  return (
    <div className="stack">
      <div className="row-between">
        <span className="row-sub">Control apps</span>
        <Toggle checked={apps.enabled} label="Control apps" onChange={(on) => set({ enabled: on })} />
      </div>
      {!apps.enabled ? (
        <p className="card-sub" style={{ marginTop: 0 }}>
          No app is blocked. Turn on to block specific apps, or allow only a
          chosen set. A blocked app stays blocked even during allowed time.
        </p>
      ) : (
        <div className="stack">
          <div>
            <div className="field-label">Mode</div>
            <Segmented
              value={apps.posture}
              onChange={(v) => set({ posture: v })}
              options={[
                { value: "blocklist", label: "Block listed apps" },
                { value: "allowlist", label: "Only allow listed" },
              ]}
            />
          </div>

          {(sections ? sections.length > 0 : available.length > 0) ? (
            <div>
              <div className="field-label">{isAllow ? "Allowed apps" : "Blocked apps"}</div>
              <p className="card-sub" style={{ margin: "2px 0 8px" }}>
                Tap an app to {verb} it{isAllow ? " (everything else is blocked)" : ""}.
              </p>
              {/* "Everything else" means something narrower on a computer, and
                  saying so is better than a parent discovering it. charterd
                  stops the apps the machine lists (`apps_policy.rs` — the same
                  list shown above); a laptop can always run something that was
                  never on it, like a download or a script. A phone-only family
                  never sees this. */}
              {isAllow && coversComputer && (
                <p className="card-sub" style={{ margin: "0 0 8px" }}>
                  On a computer this covers the apps listed here. A program they
                  download or write themselves isn’t in the list, so it isn’t
                  stopped — worth a conversation rather than a setting.
                </p>
              )}
              {sections ? (
                // Grouped by device — the merged list mixes a laptop's flatpak
                // ids in with a phone's Android packages as though they were
                // one vocabulary, and a guardian toggling one need to know
                // which device it actually reaches.
                sections.map((s) => (
                  <div key={s.deviceId} style={{ marginBottom: 14 }}>
                    <div className="row-sub" style={{ marginBottom: 4 }}>
                      {s.platform === "android" ? "📱" : "💻"} {s.label}
                    </div>
                    {s.apps.length === 0 ? (
                      <p className="card-sub" style={{ margin: 0 }}>
                        No apps reported from this device yet — they’ll appear
                        here once it syncs. You can add package names below in
                        the meantime.
                      </p>
                    ) : (
                      <div className="stack" style={{ gap: 2 }}>
                        {s.apps.map(renderAppRow)}
                      </div>
                    )}
                  </div>
                ))
              ) : (
                <div className="stack" style={{ gap: 2 }}>
                  {available.map(renderAppRow)}
                </div>
              )}
            </div>
          ) : (
            <p className="card-sub" style={{ marginTop: 0 }}>
              No apps reported from the device yet — they’ll appear here once it
              syncs. You can add package names below in the meantime.
            </p>
          )}

          <DomainList
            label={available.length > 0 ? "Other apps (by package name)" : `Apps to ${verb}`}
            hint={`Add any package name (e.g. ${isAllow ? "org.mozilla.fenix" : "com.google.android.youtube"}).`}
            items={extras}
            placeholder={isAllow ? "org.mozilla.fenix" : "com.google.android.youtube"}
            normalize={(s) => s.trim()}
            reject={isCmdlineIdentity}
            rejectMessage="Kintrinsic attaches this kind of identity for supported apps itself (like Minecraft) — you can't type it directly."
            onChange={(nextExtras) => setList([...list.filter((p) => reportedPkgs.has(p)), ...nextExtras])}
          />
        </div>
      )}

      {/* Removing an app is a different decision from blocking one — the junk a
          tablet ships with is not a rule about the child's day — so it sits
          OUTSIDE the "Control apps" switch above and stays available even when
          app control is off. Since ward 0.6.9 locks USB debugging by design,
          this is the only route a guardian has left to take OEM bloat off a
          device at all. */}
      {hide && (
        <div style={{ marginTop: 4 }}>
          <div className="field-label">Remove from device</div>
          <p className="card-sub" style={{ margin: "2px 0 8px" }}>
            Removed apps disappear from the device as if uninstalled — good for
            the junk a tablet ships with. You can put them back any time.
          </p>
          {/* Named devices, never a count, and said BEFORE the press rather
              than discovered after nothing happens — the same shape every
              other gate on this screen uses. */}
          {hide.tooOldNote && (
            <p className="card-sub" style={{ margin: "0 0 8px" }}>{hide.tooOldNote}</p>
          )}
          {hide.parityNote && (
            <p className="card-sub" style={{ margin: "0 0 8px" }}>{hide.parityNote}</p>
          )}
          {removed.length > 0 && (
            <div style={{ marginBottom: 12 }}>
              <div className="row-sub" style={{ marginBottom: 4 }}>Removed</div>
              <div className="stack" style={{ gap: 2 }}>
                {removed.map((pkg) => (
                  <div className="row-between" key={pkg} style={{ padding: "4px 0" }}>
                    <span>
                      {labelForPkg(pkg)}
                      <span className="muted" style={{ fontSize: "var(--fs-small)", marginLeft: 8 }}>
                        {pkg}
                      </span>
                    </span>
                    {/* Never disabled by the gate above: putting an app BACK
                        is the direction that can only ever help, and a
                        guardian must be able to undo a removal even while
                        some device is too old to have honoured it. */}
                    <button
                      type="button"
                      className="btn btn-secondary"
                      aria-label={`Put ${labelForPkg(pkg)} back on the device`}
                      style={{ padding: "4px 10px", minWidth: 0, lineHeight: 1.1 }}
                      onClick={() => restore(pkg)}
                    >
                      Put back
                    </button>
                  </div>
                ))}
              </div>
            </div>
          )}
          {available.length === 0 ? (
            <p className="card-sub" style={{ margin: 0 }}>
              No apps reported from the device yet — they’ll appear here once it
              syncs.
            </p>
          ) : (
            <div className="stack" style={{ gap: 2 }}>
              {available
                .filter((a) => !removedSet.has(a.pkg))
                .map((a) => (
                  <div className="row-between" key={a.pkg} style={{ padding: "4px 0" }}>
                    <span>
                      {a.label}
                      <span className="muted" style={{ fontSize: "var(--fs-small)", marginLeft: 8 }}>
                        {a.pkg}
                      </span>
                    </span>
                    <button
                      type="button"
                      className="btn btn-secondary"
                      disabled={!hide.canRemove}
                      aria-label={`Remove ${a.label} from the device`}
                      style={{ padding: "4px 10px", minWidth: 0, lineHeight: 1.1 }}
                      onClick={() => setHidden([...hiddenList, a.pkg])}
                    >
                      Remove
                    </button>
                  </div>
                ))}
            </div>
          )}
        </div>
      )}

      {hold && holdFor_ && openApp && (
        <AppHoldSheet
          label={openApp.label}
          direction={holdDirection(apps, holdFor_, hold.nowUnix)}
          live={holdFor(apps, holdFor_, hold.nowUnix)}
          nowUnix={hold.nowUnix}
          schedule={hold.schedule}
          tooOldNote={hold.tooOldNote}
          otherChangesPending={hold.otherChangesPending}
          busy={hold.saving}
          onPick={(untilUnix) => {
            // Through the SAME onChange the toggle uses, so a split control's
            // hold lands on the device it was set for…
            onChange(
              putHold(
                apps,
                holdFor_,
                holdDirection(apps, holdFor_, hold.nowUnix),
                untilUnix,
                hold.nowUnix,
              ),
            );
            // …and then the ordinary save signs it. One press for the guardian,
            // one signed clause on the wire.
            hold.requestSave();
            setHoldFor(null);
          }}
          onEnd={() => {
            onChange(putHold(apps, holdFor_, "allowed", null, hold.nowUnix));
            hold.requestSave();
            setHoldFor(null);
          }}
          onCancel={() => setHoldFor(null)}
        />
      )}
    </div>
  );
}

/** Everything `Split` needs from `DeviceLimits`, passed as one prop so Split
 *  can live at MODULE scope. Defined inside the component, its function
 *  identity changed on every parent render, and React — which matches elements
 *  by component reference — unmounted and remounted the entire subtree under
 *  every `<Split>` each time. `DeviceLimits` re-renders on every store context
 *  change (a heartbeat lands ~every 60s per live phone), so editor state was
 *  wiped mid-edit: the per-day toggle snapped back, the collapse-confirm
 *  banner vanished, half-typed block-list entries were lost. */
interface SplitCtx {
  canSplit: boolean;
  pairedDevices: Device[];
  isSplit: (control: SplittableControl) => boolean;
  splitOn: (control: SplittableControl, shared: unknown) => void;
  splitOff: (control: SplittableControl) => void;
  overrideFor: (deviceId: string, control: SplittableControl) => unknown;
  setDeviceValue: (deviceId: string, control: SplittableControl, value: unknown) => void;
}

/**
 * Renders a control either once (shared) or once per device (split), with the
 * toggle between them. `render` receives the value and a setter, so each
 * editor stays exactly the component it already was.
 */
function Split<V>({
  ctx,
  control,
  shared,
  setShared,
  render,
}: {
  ctx: SplitCtx;
  control: SplittableControl;
  shared: V;
  setShared: (v: V) => void;
  /** `deviceId` is the device this copy is for, or undefined for the one shared
   *  rule. An editor that offers a list of the ward's apps MUST use it — the
   *  shared list spans every device, and half of it can't exist on any one of
   *  them. */
  render: (value: V, onChange: (v: V) => void, deviceId?: string) => ReactNode;
}) {
  const { canSplit, pairedDevices, isSplit, splitOn, splitOff, overrideFor, setDeviceValue } =
    ctx;
  const split = isSplit(control);
  return (
    <>
      {canSplit && (
        // The label must say what turning it ON does, and never change with the
        // state. It used to read "Same on every device" beside an OFF toggle,
        // which says the opposite of what is true — a guardian reads the switch
        // as applying to the words next to it (decented, 2026-08-01). The current
        // state belongs underneath, as a readout, not in the label.
        <div className="row-between" style={{ marginBottom: 10 }}>
          <div className="row-main">
            <span className="row-title">Set separately for each device</span>
            <br />
            <span className="row-sub">
              {split
                ? "Each device has its own — edit them below."
                : "Off: every device follows the one rule."}
            </span>
          </div>
          <Toggle
            checked={split}
            label="Set separately for each device"
            onChange={(on) => (on ? splitOn(control, shared) : splitOff(control))}
          />
        </div>
      )}
      {split && canSplit
        ? pairedDevices.map((d) => (
            <div key={d.id} style={{ marginBottom: 14 }}>
              <div className="row-sub" style={{ marginBottom: 4 }}>
                {d.platform === "android" ? "📱" : "💻"} {d.label}
              </div>
              {render(
                (overrideFor(d.id, control) as V | undefined) ?? shared,
                (v) => setDeviceValue(d.id, control, v),
                d.id,
              )}
            </div>
          ))
        : render(shared, setShared)}
    </>
  );
}

function DeviceLimits({
  child,
  policy,
  section,
  onDirty,
  openSection,
  onOpenSection,
}: {
  child: Child;
  policy: Policy;
  /** Which second-level tab is showing. Owned by the screen, because the
   *  Content tab also carries the per-app policy cards, which live out here. */
  section: SectionTab;
  /** Comma-joined ids of the sections with unsaved changes, reported upward so
   *  the tab strip can dot a tab you cannot currently see. */
  onDirty: (key: string) => void;
  /** The one open row, or null. Lifted so the save bar can open the row holding
   *  an unsaved change. */
  openSection: string | null;
  onOpenSection: (id: string | null) => void;
}) {
  const { savePolicy, state, deviceStatus } = useCharter();
  // Limits the ward's own machine is enforcing that were set ON the machine.
  // Without this the screen shows "not selected" while the device enforces two
  // hours, and gives no warning that saving here discards them wholesale.
  const onDeviceLimits = useMemo(
    () => deviceSetLimits(child, deviceStatus),
    [child, deviceStatus],
  );
  // Ship-order guard: only offer the v2 lifeline once EVERY paired phone
  // reports a versionCode that accepts it. An older phone drops a v2 clause
  // wholesale (fail-closed), which would leave the ward unable to call
  // anyone — the exact failure this feature exists to prevent. Self-lifting
  // as devices update; a phone that hasn't reported yet counts as not-ready.
  const lifelineV2Ready = useMemo(() => {
    const phones = child.devices.filter((d) => d.platform === "android" && d.devicePubkey);
    if (phones.length === 0) return false;
    return phones.every(
      (d) => (deviceStatus[d.devicePubkey as string]?.appVersionCode ?? 0) >=
        LIFELINE_V2_MIN_VERSION_CODE,
    );
  }, [child.devices, deviceStatus]);

  const pairedDevices = child.devices.filter((d) => d.pairing === "paired");

  // Apps the child's paired devices have reported (D3 picker source), kept BOTH
  // ways round: per device for a split rule, merged for a shared one. See
  // domain/deviceApps — offering the merged list to a split editor writes rules
  // that can never fire on the device they are under.
  const byDevice = useMemo(
    () => appsByDevice(child.devices, deviceStatus),
    [child.devices, deviceStatus],
  );
  const reportedApps = useMemo(() => mergedApps(byDevice), [byDevice]);
  const appsOn = (deviceId?: string) => appsFor(byDevice, reportedApps, deviceId);
  // The SAME inventory as `reportedApps`, grouped one section per paired
  // device — for a picker over the MERGED (unsplit) list, so a guardian can
  // still see which device an app lives on. A picker already scoped to one
  // device (the Split per-device render below) has no use for this — it
  // already shows only that device's own apps under that device's own
  // heading, so passing sections there would just repeat it.
  const appSections = useMemo(
    () => appsBySection(child.devices, deviceStatus),
    [child.devices, deviceStatus],
  );

  /** Would this rule land on a COMPUTER? An allowlist means something weaker
   *  there than on a phone — charterd stops the apps the machine lists, and a
   *  laptop can always run something it never listed (a download, a script).
   *  The caveat is only shown where it is true, so a phone-only family never
   *  reads a warning about a machine they don't own. */
  const coversComputer = (deviceId?: string) =>
    child.devices.some((d) => (deviceId ? d.id === deviceId : true) && d.platform === "linux");

  /**
   * Devices reporting that they have educational sites switched on but no
   * browser to open them in. Named rather than counted, because "install
   * Chromium" is an instruction that needs a machine attached to it.
   */
  const runtimeMissingOn = (deviceId?: string) =>
    child.devices
      .filter((d) => (deviceId ? d.id === deviceId : true))
      .filter((d) => deviceStatus[d.devicePubkey as string]?.siteRuntimeMissing)
      .map((d) => d.label);
  // Says what the rules actually cover, in the family's nouns — a phone is a
  // computer to us and not to a parent (see domain/deviceWords).
  const deviceNote = deviceScopeNote(child.name, pairedDevices);

  const [draftSchedule, setDraftSchedule] = useState<Schedule>(() =>
    buildSchedule(policy.schedule),
  );
  const [draftBudget, setDraftBudget] = useState<Budget>(() =>
    buildBudget(policy.budget),
  );
  const [draftWeb, setDraftWeb] = useState<WebPolicy>(() => buildWeb(policy.web));
  // The classic Apps section's MANUAL layer only — never contains what named
  // times' on-request groups add (see `effectiveApps` below, which layers the
  // two together for dirty-check/save without either overwriting the other).
  const [draftApps, setDraftApps] = useState<AppsPolicy>(() => buildApps(policy.apps));
  const [draftTethering, setDraftTethering] = useState<Tethering>(() =>
    buildTethering(policy.tethering),
  );
  const [draftLifeline, setDraftLifeline] = useState<Lifeline>(() =>
    buildLifeline(policy.lifeline),
  );
  // ── Named times (learning + buckets, unified) ───────────────────────────
  // `draftGroups`/`draftLearningEnabled`/`draftBucketsEnabled` are the ONLY
  // things the guardian edits here; the wire-shaped learning/buckets values
  // are DERIVED from them below (`compiled.learning`/`compiled.buckets`),
  // never their own state — the two old sections are gone, so nothing else
  // writes to either dimension. All three are seeded from the SAME
  // `decompose` call `savedDraft` (below) uses, so a freshly-mounted screen
  // is trivially clean (see the C1 review finding this fixes: comparing a
  // freshly-derived value against a differently-derived "saved" value is
  // what made an untouched screen read as dirty forever).
  const namedTimesPrior = { learning: policy.learning, buckets: policy.buckets, apps: policy.apps };
  const [draftGroups, setDraftGroups] = useState<NamedGroup[]>(
    () => decompose(namedTimesPrior, policy.freeGroups).groups,
  );
  const [draftLearningEnabled, setDraftLearningEnabled] = useState<boolean>(
    () => decompose(namedTimesPrior, policy.freeGroups).learningEnabled,
  );
  const [draftBucketsEnabled, setDraftBucketsEnabled] = useState<boolean>(
    () => decompose(namedTimesPrior, policy.freeGroups).bucketsEnabled,
  );
  // The guardian-side pool that lets a brand-new custom site (added this
  // session, before its first save) resolve to its full domains/url rather
  // than a bare native fallback — see `domain/namedTimes.learningAppPool`.
  const [draftLearningApps, setDraftLearningApps] = useState<LearningAppSel[]>(
    () => policy.learning?.apps ?? [],
  );
  // The one knob named times still treats as shared-not-per-group (an
  // advanced passthrough, per the spec).
  const [draftCapMinutes, setDraftCapMinutes] = useState<number | undefined>(
    () => policy.learning?.capMinutes ?? undefined,
  );
  const onAddLearningApp = (app: LearningAppSel) =>
    setDraftLearningApps((prev) => (prev.some((a) => a.id === app.id) ? prev : [...prev, app]));
  const [draftListening, setDraftListening] = useState<ListeningPolicy>(() =>
    buildListening(policy.listening),
  );
  const [draftAlwaysAvailable, setDraftAlwaysAvailable] = useState<AlwaysAvailablePolicy>(() =>
    buildAlwaysAvailable(policy.alwaysAvailable),
  );
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // ── Per-device rules (Half A) ────────────────────────────────────────────
  // One charter, split where it fits the child. None of this exists for a
  // one-device family: `canSplit` gates every affordance below, so the screen
  // they see is exactly the screen they saw before this shipped.
  const [draftOverrides, setDraftOverrides] = useState<Record<string, PolicyOverride>>(
    () => policy.deviceOverrides ?? {},
  );
  const canSplit = pairedDevices.length > 1;
  const splitIds = pairedDevices.map((d) => d.id);

  const isSplit = (control: SplittableControl) =>
    Object.values(draftOverrides).some((o) => o[control] !== undefined);

  /** Splitting seeds every device with the CURRENT DRAFT value, so switching
   *  it on changes nothing until the parent edits one of them. */
  function splitOn(control: SplittableControl, shared: unknown) {
    const pseudo = {
      ...policy,
      [control]: shared,
      deviceOverrides: draftOverrides,
    } as Policy;
    setDraftOverrides(setControlSplit(pseudo, control, splitIds).deviceOverrides ?? {});
  }

  function splitOff(control: SplittableControl) {
    const pseudo = { ...policy, deviceOverrides: draftOverrides } as Policy;
    setDraftOverrides(setControlShared(pseudo, control).deviceOverrides ?? {});
  }

  function setDeviceValue(deviceId: string, control: SplittableControl, value: unknown) {
    setDraftOverrides((prev) => ({
      ...prev,
      [deviceId]: { ...prev[deviceId], [control]: value },
    }));
  }

  // The module-scope `Split` reads everything through this (see SplitCtx). A
  // fresh object per render is fine — what must stay stable is the COMPONENT
  // identity, not the props.
  const splitCtx: SplitCtx = {
    canSplit,
    pairedDevices,
    isSplit,
    splitOn,
    splitOff,
    overrideFor: (deviceId, control) => draftOverrides[deviceId]?.[control],
    setDeviceValue,
  };

  const savedSchedule = useMemo(
    () => buildSchedule(policy.schedule),
    [policy.schedule],
  );
  const savedBudget = useMemo(
    () => buildBudget(policy.budget),
    [policy.budget],
  );
  const savedWeb = useMemo(() => buildWeb(policy.web), [policy.web]);
  const savedApps = useMemo(() => buildApps(policy.apps), [policy.apps]);
  const savedTethering = useMemo(
    () => buildTethering(policy.tethering),
    [policy.tethering],
  );
  const savedLifeline = useMemo(
    () => buildLifeline(policy.lifeline),
    [policy.lifeline],
  );

  // Has a schedule ever actually been signed for this ward? `buildSchedule`
  // normalises "no clause at all" into the same seven empty days that mean
  // "deliberately off", so without this the two are indistinguishable — and a
  // brand-new ward showed "Off unless I open it" already ON, with nothing to
  // save, over a device nothing was governing (Blue Tablet, 2026-07-29).
  const scheduleAuthored = policy.schedule !== undefined;
  // The guardian has ASKED for dormancy on a ward that has no schedule yet.
  // Their intent is a real change even though the draft already looks empty,
  // so it must make the card dirty — otherwise Save stays greyed out and the
  // clause is never signed.
  const [dormantIntent, setDormantIntent] = useState(false);
  const scheduleDirty =
    dormantIntent || JSON.stringify(draftSchedule) !== JSON.stringify(savedSchedule);
  const budgetDirty =
    JSON.stringify(draftBudget) !== JSON.stringify(savedBudget);
  const webDirty = JSON.stringify(draftWeb) !== JSON.stringify(savedWeb);
  const tetheringDirty =
    JSON.stringify(draftTethering) !== JSON.stringify(savedTethering);
  const lifelineDirty =
    JSON.stringify(draftLifeline) !== JSON.stringify(savedLifeline);
  // Splitting a control, or editing one device's copy of it, is a real change
  // even when the shared value is untouched — otherwise Save stays greyed out
  // and the parent's edit silently goes nowhere. (`overridesDirty`/
  // `overrideDirty` themselves are defined further down, once `compiled` and
  // `effectiveOverrides` exist — every control but "apps" reads the raw draft
  // through them unchanged; only "apps" is fragment-aware.)
  const savedListening = useMemo(() => buildListening(policy.listening), [policy.listening]);
  const listeningDirty = JSON.stringify(draftListening) !== JSON.stringify(savedListening);
  const savedAlwaysAvailable = useMemo(
    () => buildAlwaysAvailable(policy.alwaysAvailable),
    [policy.alwaysAvailable],
  );
  const alwaysAvailableDirty =
    JSON.stringify(draftAlwaysAvailable) !== JSON.stringify(savedAlwaysAvailable);

  // ── Named times: compile the draft down to the three fragments one save
  // signs. `buckets`' tz has no editor of its own (never has) — fall back to
  // the device-local zone the same way the old `buildBuckets` did, for a
  // family that has never touched it before.
  const bucketsWeekStart = policy.buckets?.weekStart ?? policy.budget?.weekStart;
  const bucketsTzPrior: BucketsPolicy = useMemo(
    () => ({
      enabled: false,
      tz: policy.buckets?.tz || LOCAL_TZ,
      buckets: [],
      // The weekly named-time pool must roll over on the SAME day as the
      // weekly budget. Nothing ever wrote `buckets.weekStart`, so the clause
      // omitted it and the ward defaulted to Monday, while the budget clause
      // always says "sun" — two weekly meters resetting on different days.
      // Feeds BOTH compiles below, so it never reads as an unsaved change; an
      // existing clause picks it up the next time named times are saved.
      ...(bucketsWeekStart ? { weekStart: bucketsWeekStart } : {}),
    }),
    [policy.buckets?.tz, bucketsWeekStart],
  );
  const namedTimesCompilePrior = useMemo(
    () => ({
      learning: {
        enabled: false,
        apps: draftLearningApps,
        ...(draftCapMinutes != null ? { capMinutes: draftCapMinutes } : {}),
      },
      buckets: bucketsTzPrior,
      apps: policy.apps,
    }),
    [draftLearningApps, draftCapMinutes, bucketsTzPrior, policy.apps],
  );
  const compiled = useMemo(
    () =>
      groupsToClauses(
        { groups: draftGroups, learningEnabled: draftLearningEnabled, bucketsEnabled: draftBucketsEnabled },
        namedTimesCompilePrior,
      ),
    [draftGroups, draftLearningEnabled, draftBucketsEnabled, namedTimesCompilePrior],
  );
  // What compiling the SAVED draft back down produces — the round-trip
  // baseline `compiled` is compared against. Built the IDENTICAL way as the
  // mount-time draft (same `decompose` call, same prior) — the C1 review
  // finding was exactly this comparison drifting apart (comparing a freshly
  // compiled value against a raw, differently-shaped saved field), which made
  // an untouched screen read as permanently dirty and, on save, sign a
  // never-existed empty buckets clause. `PriorClauseState`'s SAVED shape
  // (`policy.learning` verbatim, not the draft pool) is what this recompiles
  // against, so it is byte-true to what is actually on the wire right now.
  const savedNamedTimesPrior = useMemo(
    () => ({ learning: policy.learning, buckets: policy.buckets, apps: policy.apps }),
    [policy.learning, policy.buckets, policy.apps],
  );
  const savedDraft = useMemo(
    () => decompose(savedNamedTimesPrior, policy.freeGroups),
    [savedNamedTimesPrior, policy.freeGroups],
  );
  const savedNamedTimesCompilePrior = useMemo(
    () => ({ learning: policy.learning, buckets: bucketsTzPrior, apps: policy.apps }),
    [policy.learning, bucketsTzPrior, policy.apps],
  );
  const savedCompiled = useMemo(
    () => groupsToClauses(savedDraft, savedNamedTimesCompilePrior),
    [savedDraft, savedNamedTimesCompilePrior],
  );
  // Per-dimension, so a save signs ONLY what actually changed — the other
  // half of C1: a family with no buckets clause who only edits a free group
  // must never have `buckets` ride along in the save payload just because
  // SOME named-times dimension changed.
  const learningDirty = JSON.stringify(compiled.learning) !== JSON.stringify(savedCompiled.learning);
  const bucketsDirty = JSON.stringify(compiled.buckets) !== JSON.stringify(savedCompiled.buckets);
  const freeGroupsDirty = JSON.stringify(compiled.freeGroups) !== JSON.stringify(savedCompiled.freeGroups);
  const namedTimesDirty = learningDirty || bucketsDirty || freeGroupsDirty;
  const namedTimesErr = namedTimesDirty ? namedTimesError(draftGroups) : null;

  // Every pkg a save-worthy named-times on-request group currently owns —
  // the classic Apps section must never show these as ordinary removable
  // pills (I3 review finding: tapping one there looked like it worked, but
  // `applyAppsFragment` re-added it from the still-live group on the very
  // next render, and the row never even showed as dirty).
  const managedAppsPkgs = useMemo(() => new Set(compiled.apps.askFirst), [compiled.apps.askFirst]);

  // The classic Apps section's manual layer, with named times' on-request
  // groups layered on top — the exact value a save signs for the `apps`
  // dimension. Computed fresh each render so removing a group correctly
  // lifts what named times added without touching a manual block.
  const effectiveApps = useMemo(
    () => applyAppsFragment(draftApps, compiled.apps),
    [draftApps, compiled.apps],
  );
  const appsDirty = JSON.stringify(effectiveApps) !== JSON.stringify(savedApps);

  // I4(a): a device that splits "apps" gets its OWN override signed — the
  // base alone is not enough, `effectivePolicyForDevice` picks EITHER the
  // base OR a split device's override, never both. Applying the identical
  // fragment to every split override is additive and idempotent (see
  // `applyAppsFragment`'s doc), so composing it here is safe to do
  // unconditionally; only devices that already split "apps" are touched.
  const effectiveOverrides = useMemo(() => {
    let changed = false;
    const out: Record<string, PolicyOverride> = {};
    for (const [deviceId, ov] of Object.entries(draftOverrides)) {
      if (ov.apps !== undefined) {
        const nextApps = applyAppsFragment(ov.apps, compiled.apps);
        out[deviceId] = { ...ov, apps: nextApps };
        if (JSON.stringify(nextApps) !== JSON.stringify(ov.apps)) changed = true;
      } else {
        out[deviceId] = ov;
      }
    }
    return changed ? out : draftOverrides;
  }, [draftOverrides, compiled.apps]);
  const overridesDirty =
    JSON.stringify(effectiveOverrides) !== JSON.stringify(policy.deviceOverrides ?? {});
  const overrideDirty = (c: SplittableControl) =>
    JSON.stringify(pickOverrides(effectiveOverrides, [c])) !==
    JSON.stringify(pickOverrides(policy.deviceOverrides, [c]));

  // I4(b): named times has no split UI of its own, but a LEGACY per-device
  // learning override (from before this dimension was unified) or a live
  // per-device apps split both mean SOME devices keep their own separate
  // rule that base edits here will never reach — say so plainly rather than
  // silently folding or deleting either kind of override.
  const learningSplitDevices = isControlSplit(policy, "learning")
    ? splitDeviceIds(policy, "learning")
        .map((id) => child.devices.find((d) => d.id === id)?.label)
        .filter((label): label is string => Boolean(label))
    : [];
  const appsSplitDevices = isControlSplit(policy, "apps")
    ? splitDeviceIds(policy, "apps")
        .map((id) => child.devices.find((d) => d.id === id)?.label)
        .filter((label): label is string => Boolean(label))
    : [];

  const dirty =
    scheduleDirty || budgetDirty || webDirty || appsDirty ||
    tetheringDirty || lifelineDirty || overridesDirty || namedTimesDirty || listeningDirty ||
    alwaysAvailableDirty;
  const lifelineError = lifelineDirty ? lifelineNumberError(draftLifeline) : null;
  // Block a save that would throw at sign time (or, unsigned, be stored as a
  // window that can never be "in") — surface it in the editor instead.
  const scheduleError = scheduleWindowError(draftSchedule);

  // --- app holds ---------------------------------------------------------
  //
  // A hold counts down on screen, so the clock has to move — but only while the
  // Apps section is actually open, and only while the tab is visible.
  //
  // Gating this on "…and a hold is live" was wrong and worth remembering why: a
  // screen left open for an hour with nothing counting would hold a clock an
  // hour stale, and the first hold set from it would resolve to an instant in
  // the PAST — pruned on the way to the wire, so the guardian's press would
  // silently do nothing. A clock read only when it is right is not a clock.
  const holdNow = useNowUnix(openSection === "apps");
  // A hold is applied through the ordinary draft, then saved by the ordinary
  // save — bumping this asks for the second half. It cannot call `save()`
  // inline: React has not yet re-rendered with the new draft, so `save` would
  // close over the OLD `draftApps` and sign a clause without the hold in it.
  // The effect below runs after the draft has settled.
  const [holdSaveRequests, setHoldSaveRequests] = useState(0);
  useEffect(() => {
    if (holdSaveRequests === 0) return;
    void save();
    // `save` is re-created every render and deliberately not a dependency: this
    // must fire once per request, on the render that carries the new draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [holdSaveRequests]);

  // Naming only devices that actually REPORT a Kintrinsic too old — silence is not
  // incapacity, and a phone that is merely switched off still gets the clause.
  const holdSupport = useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "appHold"),
    [child.devices, deviceStatus],
  );

  // alwaysAvailable is version-gated on Android (39) as well as NEVER on
  // Linux (the `unsupported` half is covered by `parityNote` below) — every
  // ward on the branch before this one reports 38, so on the day this ships
  // EVERY existing phone is `tooOld`. Without naming that, a guardian would
  // name an app here, see nothing wrong, and watch nothing happen on the
  // phone: the same silent-failure shape `parityNote` exists to prevent.
  const alwaysAvailableSupport = useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "alwaysAvailable"),
    [child.devices, deviceStatus],
  );
  const alwaysAvailableTooOldNote = tooOldNote(
    alwaysAvailableSupport,
    "keep an app open at any hour",
  );

  /**
   * The note under a control some of this ward's devices will NEVER honour —
   * six of the fourteen clauses are stored-then-ignored on one platform or the
   * other, and until the 2026-08-02 audit nothing anywhere said so. A guardian
   * setting "Play is an hour a day" for a phone-only ward was authoring a rule
   * that did not exist. Unlike the too-old note this can never lift by itself,
   * so it states the consequence plainly and says what governs there instead.
   */
  const parityNote = (
    feature: WardenFeature,
    what: string,
    instead?: string,
    untilUpdated?: string,
  ) =>
    supportNote(
      wardenSupport(deliverableDevices(child.devices), deviceStatus, feature),
      what,
      instead,
      untilUpdated,
    );
  // The NEVER half alone — for the two controls that already render their own
  // too-old sentence beside it (always-available, removing an app).
  const neverNote = (feature: WardenFeature, what: string, instead?: string) =>
    unsupportedNote(
      wardenSupport(deliverableDevices(child.devices), deviceStatus, feature),
      what,
      instead,
    );

  // Honest attribution: whether EVERY deliverable device supports the
  // `cmdline:` identity form. Unlike `parityNote`'s usual "can send while
  // SOME device honours it" (`canSend`), attaching a `cmdline:` identity is
  // safe only when NOTHING would silently ignore it — an old warden treats
  // the string as inert, which loosens a BLOCKED list rather than tightening
  // it (see `domain/launchSignatures.ts`'s file doc).
  const cmdlineSupport = useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "cmdlineIdentity"),
    [child.devices, deviceStatus],
  );
  const cmdlineOk = cmdlineSupport.tooOld.length === 0 && cmdlineSupport.unsupported.length === 0;
  // The device that actually LOSES something when `cmdlineOk` is false is
  // whichever Linux device(s) the ward has — the identity is a Linux exec
  // path/flatpak-id concept that can never match an Android package, so an
  // incapable PHONE is the REASON nothing attaches, never the device the
  // guardian should be told to look at (found in review, F2: an earlier
  // draft said the phone "still covers Minecraft by its usual name", which
  // is meaningless for a platform this identity was never going to reach).
  const cmdlineLinuxNames = deliverableDevices(child.devices)
    .filter((d) => d.platform === "linux")
    .map((d) => d.label)
    .join(" and ");
  const cmdlineFallbackClause = cmdlineLinuxNames
    ? ` On ${cmdlineLinuxNames}, Minecraft is only covered when it's started from this launcher.`
    : "";
  const cmdlineParityNote =
    cmdlineSupport.unsupported.length > 0
      ? `${cmdlineSupport.unsupported.map((d) => d.label).join(" and ")} can't support the extra identity Kintrinsic attaches for Minecraft, so Kintrinsic isn't attaching it at all.${cmdlineFallbackClause}`
      : cmdlineSupport.tooOld.length > 0
        ? `${cmdlineSupport.tooOld.map((d) => d.label).join(" and ")} needs the latest Kintrinsic before Minecraft gets its extra identity — until then, Kintrinsic isn't attaching it at all.${cmdlineFallbackClause}`
        : null;

  // "Remove from device" is Android Device Owner only and brand new (ward
  // Kintrinsic 0.6.10, versionCode 41), so on the day it ships EVERY phone
  // already out there reports too old and every laptop is a permanent NEVER —
  // exactly the two failure shapes this screen names devices for rather than
  // letting a guardian press a button and watch nothing happen.
  const appHideSupport = useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "appHide"),
    [child.devices, deviceStatus],
  );
  const hideCtx: HideCtx = {
    canRemove: appHideSupport.canSend,
    tooOldNote: tooOldNote(appHideSupport, "remove an app from the device"),
    parityNote: neverNote(
      "appHide",
      "Removing an app",
      "A computer has no launcher to hide an app from — uninstall it there the ordinary way.",
    ),
  };

  const holdCtx: HoldCtx = {
    nowUnix: holdNow,
    schedule: scheduleAuthored ? draftSchedule : undefined,
    tooOldNote: holdSupport.tooOld.length
      ? `${holdSupport.tooOld.map((d) => d.label).join(" and ")} needs the latest Kintrinsic before it can hold an app. Update it, then this starts working by itself.`
      : null,
    otherChangesPending: dirty,
    requestSave: () => setHoldSaveRequests((n) => n + 1),
    saving,
  };

  async function save() {
    if (scheduleError) {
      setSaveError(scheduleError);
      return;
    }
    if (lifelineError) {
      setSaveError(lifelineError);
      return;
    }
    if (namedTimesErr) {
      setSaveError(namedTimesErr);
      return;
    }
    setSaveError(null);
    setSaving(true);
    try {
      // One signed change carrying every changed dimension (shared issuedAt), so
      // saving them together never reverts one of the others on the device.
      //
      // A control must also be signed when only its PER-DEVICE copy changed:
      // each device's effective value is base + its override, so leaving the
      // dimension out would sign nothing and the parent's edit would silently
      // never reach the phone.
      await savePolicy(child.id, policy.id, {
        schedule: scheduleDirty || overrideDirty("schedule") ? draftSchedule : undefined,
        budget: budgetDirty || overrideDirty("budget") ? draftBudget : undefined,
        web: webDirty || overrideDirty("web") ? draftWeb : undefined,
        apps: appsDirty || overrideDirty("apps") ? effectiveApps : undefined,
        // Each of these three is gated on its OWN comparison (C1 fix) — a
        // family who only edits a free group must never have an empty
        // buckets clause ride along just because SOME named-times dimension
        // changed, and vice versa.
        alwaysAvailable: alwaysAvailableDirty ? draftAlwaysAvailable : undefined,
        learning: learningDirty ? compiled.learning : undefined,
        buckets: bucketsDirty ? compiled.buckets : undefined,
        freeGroups: freeGroupsDirty ? compiled.freeGroups : undefined,
        listening: listeningDirty ? draftListening : undefined,
        // Fragment-applied (I4a) — a device that splits "apps" gets the same
        // on-request pkgs the base does, not just whatever it had before.
        deviceOverrides: effectiveOverrides,
        tethering: tetheringDirty ? draftTethering : undefined,
        lifeline: lifelineDirty
          ? {
              // Carry the WHOLE draft. Listing only `numbers` here silently
              // dropped emergencyServices, breakGlass and torch on every save
              // — the guardian ticked "Emergency unlock", saved, and the flag
              // never reached the phone. Spread first, then filter, so adding
              // a future knob can't reintroduce the same silent loss.
              ...draftLifeline,
              // Drop blank rows so the device's fail-closed validator never
              // sees a half-filled entry.
              numbers: draftLifeline.numbers.filter(
                (n) => n.label.trim() && n.number.trim(),
              ),
            }
          : undefined,
      });
      // The intent has become a signed clause: `policy.schedule` now exists, so
      // `scheduleAuthored` carries the toggle from here and the card can settle
      // (this is what lets Save grey out again).
      setDormantIntent(false);
    } catch (e) {
      // A signer failure (bunker timeout, rejected sign, wire validation) must
      // NOT vanish — the parent would think the change applied when it didn't.
      setSaveError(
        e instanceof Error && e.message
          ? `Couldn’t save these changes: ${e.message}`
          : "Couldn’t save these changes. Please try again.",
      );
    } finally {
      setSaving(false);
    }
  }

  function discard() {
    setDormantIntent(false);
    setDraftSchedule(buildSchedule(policy.schedule));
    setDraftBudget(buildBudget(policy.budget));
    setDraftWeb(buildWeb(policy.web));
    setDraftApps(buildApps(policy.apps));
    {
      const reset = decompose(
        { learning: policy.learning, buckets: policy.buckets, apps: policy.apps },
        policy.freeGroups,
      );
      setDraftGroups(reset.groups);
      setDraftLearningEnabled(reset.learningEnabled);
      setDraftBucketsEnabled(reset.bucketsEnabled);
    }
    setDraftLearningApps(policy.learning?.apps ?? []);
    setDraftCapMinutes(policy.learning?.capMinutes ?? undefined);
    setDraftTethering(buildTethering(policy.tethering));
    setDraftOverrides(policy.deviceOverrides ?? {});
    setDraftListening(buildListening(policy.listening));
    setDraftAlwaysAvailable(buildAlwaysAvailable(policy.alwaysAvailable));
    // The lifeline was the one draft Discard forgot: an abandoned edit stayed
    // dirty and rode out on the NEXT save of some unrelated dimension, so a
    // number the guardian had thought better of reached the phone anyway.
    setDraftLifeline(buildLifeline(policy.lifeline));
    setSaveError(null);
  }

  const willConfirm =
    state.signer.connected && !state.signer.autoSign && dirty;

  const dormantOn = dormantIntent || (scheduleAuthored && isDormant(draftSchedule));

  // Every control that used to be a flat `SectionLabel` heading, now as one
  // collapsed row with a summary of what it currently says. Same editors, same
  // props, same order within a group — only the wrapping changed.
  const sections: {
    id: SectionId;
    summary: string;
    dirty: boolean;
    body: ReactNode;
  }[] = [
    {
      id: "schedule",
      summary: scheduleSummary(draftSchedule, scheduleAuthored || dormantIntent),
      dirty: scheduleDirty || overrideDirty("schedule"),
      body: (
        <>
          {/* The posture comes FIRST because it overrules everything under it: a
              spare device that is off until someone opens it has no daily
              schedule to speak of, and showing one implies a standing allowance
              that isn't there. */}
          <div className="row-between" style={{ marginBottom: 4 }}>
            <div className="row-main">
              <span className="row-title">Off unless I open it</span>
              <br />
              <span className="row-sub">
                No screen time at all until you give some. Time you give runs out
                on its own, and the device goes back to off overnight.
              </span>
            </div>
            <Toggle
              checked={dormantOn}
              label="Off unless I open it"
              onChange={(on) => {
                setDormantIntent(on && !scheduleAuthored);
                setDraftSchedule(
                  on ? toDormant(draftSchedule) : fromDormant(draftSchedule),
                );
              }}
            />
          </div>
          {!dormantOn && (
            <>
              <div className="row-between" style={{ marginBottom: 4 }}>
                <span className="row-sub">Pause the schedule — allow anytime</span>
                <Toggle
                  checked={draftSchedule.paused ?? false}
                  label="Pause the schedule"
                  onChange={(on) =>
                    setDraftSchedule({ ...draftSchedule, paused: on })
                  }
                />
              </div>
              <Split
                ctx={splitCtx}
                control="schedule"
                shared={draftSchedule}
                setShared={setDraftSchedule}
                render={(v, on) => <ScheduleEditor schedule={v} onChange={on} />}
              />
            </>
          )}
        </>
      ),
    },
    {
      id: "budget",
      summary: budgetSummary(draftBudget),
      dirty: budgetDirty || overrideDirty("budget"),
      body: (
        <Split
          ctx={splitCtx}
          control="budget"
          shared={draftBudget}
          setShared={setDraftBudget}
          render={(v, on) => (
            <BudgetEditor
              budget={v}
              onChange={on}
              onDevice={onDeviceLimits}
              timeModelNote={parityNote(
                "timeModel",
                "Naming which apps cost",
                "There, every session still costs the old way, whatever you choose here.",
              )}
            />
          )}
        />
      ),
    },
    {
      id: "named-times",
      summary: namedTimesSummary(draftGroups),
      dirty: namedTimesDirty,
      body: (
        <>
          <NamedTimesSection
            groups={draftGroups}
            onChange={setDraftGroups}
            available={reportedApps}
            sections={appSections}
            learningApps={draftLearningApps}
            onAddLearningApp={onAddLearningApp}
            runtimeMissingOn={runtimeMissingOn()}
            capMinutes={draftCapMinutes}
            onCapMinutesChange={setDraftCapMinutes}
            learningEnabled={draftLearningEnabled}
            onLearningEnabledChange={setDraftLearningEnabled}
            bucketsEnabled={draftBucketsEnabled}
            onBucketsEnabledChange={setDraftBucketsEnabled}
            error={namedTimesErr}
            wardName={child.name}
            cmdlineOk={cmdlineOk}
            cmdlineParityNote={cmdlineParityNote}
          />
          <ParityNote
            note={parityNote(
              "buckets",
              "A counted named time",
              "There, those apps come out of the day's screen time like everything else.",
            )}
          />
          <ParityNote
            note={parityNote(
              "learning",
              "A free named time",
              "There, those apps spend the day's screen time like anything else.",
            )}
          />
          {draftGroups.some((g) => g.policy === "counted" && g.weeklyMinutes != null) && (
            <ParityNote
              note={parityNote(
                "bucketsWeekly",
                "A weekly allowance",
                undefined,
                // A group with BOTH axes stays `v: 1` and an older ward just
                // ignores the weekly half. A weekly-ONLY group makes the whole
                // clause `v: 2`, which that ward rejects outright — so EVERY
                // counted named time stops being limited, not just this one.
                draftGroups.some(
                  (g) => g.policy === "counted" && g.weeklyMinutes != null && g.dailyMinutes == null,
                )
                  ? "Until then, a named time with only a weekly limit switches off every counted named time on it — give each a daily limit too, or update first."
                  : "Until then only the daily limit is honoured there.",
              )}
            />
          )}
          {draftGroups.some((g) => g.policy === "onRequest") && (
            <ParityNote
              note={parityNote(
                "appOpenAsk",
                "Asking to open an on-request app",
                "There, the app just stays blocked — there's no ask button yet.",
              )}
            />
          )}
          {/* I4(b): named times has no split UI — a device carrying a LEGACY
              per-device learning rule (from before this dimension was
              unified) keeps that rule exactly as it is; base edits made here
              never reach it. Said plainly rather than silently folded. */}
          {learningSplitDevices.length > 0 && (
            <ParityNote
              note={`${learningSplitDevices.join(" and ")} ${learningSplitDevices.length > 1 ? "keep" : "keeps"} its own separate rule for free time, set before this screen existed — changes here don’t reach it.`}
            />
          )}
          {/* A device that splits the classic Apps section still gets every
              on-request pkg added here (I4(a) — the fragment reaches its
              override too); this just says so, since it isn't obvious from
              this screen alone that a split exists. */}
          {appsSplitDevices.length > 0 && (
            <ParityNote
              note={`${appsSplitDevices.join(" and ")} ${appsSplitDevices.length > 1 ? "have" : "has"} its own separate app rules — on-request apps you add here still reach it, but the rest of its rules are set on the Apps section's per-device split.`}
            />
          )}
        </>
      ),
    },
    {
      id: "listening",
      summary: listeningSummary(draftListening),
      dirty: listeningDirty,
      body: (
        <>
          <ListeningEditor
            listening={draftListening}
            sections={appSections}
            onChange={setDraftListening}
            wardName={child.name}
          />
          <ParityNote
            note={parityNote(
              "listening",
              "Letting audio carry on past the lock",
              "A computer stops everything when it locks, so a story can't finish there.",
            )}
          />
        </>
      ),
    },
    {
      id: "always-available",
      summary: alwaysAvailableSummary(draftAlwaysAvailable, Math.floor(Date.now() / 1000)),
      dirty: alwaysAvailableDirty,
      body: (
        <>
          <AlwaysAvailableEditor
            alwaysAvailable={draftAlwaysAvailable}
            sections={appSections}
            onChange={setDraftAlwaysAvailable}
            wardName={child.name}
          />
          {alwaysAvailableTooOldNote && (
            <div style={{ marginTop: 10 }}>
              <Banner tone="warn">{alwaysAvailableTooOldNote}</Banner>
            </div>
          )}
          <ParityNote
            note={neverNote(
              "alwaysAvailable",
              "Naming an app to stay open at any hour",
              "A computer's lock freezes everything, so nothing can be exempted there.",
            )}
          />
        </>
      ),
    },
    {
      id: "web",
      summary: webSummary(draftWeb),
      dirty: webDirty || overrideDirty("web"),
      body: (
        <Split
          ctx={splitCtx}
          control="web"
          shared={draftWeb}
          setShared={setDraftWeb}
          render={(v, on) => <WebEditor web={v} onChange={on} />}
        />
      ),
    },
    {
      id: "apps",
      // The effective (manual + named-times on-request) count — what the
      // device will actually block, not just what THIS editor added.
      summary: appsSummary(effectiveApps, holdNow),
      dirty: appsDirty || overrideDirty("apps"),
      body: (
        <>
          <Split
            ctx={splitCtx}
            control="apps"
            shared={draftApps}
            setShared={setDraftApps}
            render={(v, on, deviceId) => (
              <AppsEditor
                // I3: named-times-managed pkgs are hidden here, never an
                // ordinary removable pill — the only control surface for them
                // is their group, on the Named times section. Passing the raw
                // value through unfiltered let a guardian "remove" one that
                // `applyAppsFragment` silently re-added on the very next
                // render (found in review: the row didn't even show dirty).
                apps={stripManagedApps(v, managedAppsPkgs)}
                available={appsOn(deviceId)}
                // Only the merged (unsplit) copy needs grouping — a per-
                // device copy (deviceId set) already IS one device's list,
                // headed by that device's own name just above it (Split).
                sections={deviceId === undefined ? appSections : undefined}
                onChange={on}
                hold={holdCtx}
                hide={hideCtx}
                coversComputer={coversComputer(deviceId)}
                wardName={child.name}
              />
            )}
          />
          {managedAppsPkgs.size > 0 && (
            <p className="card-sub" style={{ marginTop: 10 }}>
              {[...managedAppsPkgs]
                // A `cmdline:` identity (Minecraft's launch signature) never
                // reaches this list as a raw string — a device never
                // REPORTS one, so it always falls to this generic fallback;
                // `humanLabelFor` resolves it to a real name first.
                .map((pkg) => reportedApps.find((a) => a.pkg === pkg)?.label ?? humanLabelFor(pkg) ?? pkg)
                .join(", ")}{" "}
              {managedAppsPkgs.size > 1 ? "are" : "is"} managed in Named times — edit{" "}
              {managedAppsPkgs.size > 1 ? "them" : "it"} there, not here.
            </p>
          )}
        </>
      ),
    },
    {
      id: "lifeline",
      summary: lifelineSummary(draftLifeline),
      dirty: lifelineDirty,
      body: (
        <>
          <LifelineEditor
            lifeline={draftLifeline}
            onChange={setDraftLifeline}
            v2Ready={lifelineV2Ready}
          />
          {lifelineError && (
            <div style={{ marginTop: 8 }}>
              <Banner tone="warn">{lifelineError}</Banner>
            </div>
          )}
          <ParityNote
            note={parityNote(
              "lifeline",
              "The lock-screen lifeline",
              "A computer's lock has no call button yet, so this is a phone thing for now.",
            )}
          />
        </>
      ),
    },
    {
      id: "tethering",
      summary: tetheringSummary(draftTethering),
      dirty: tetheringDirty,
      body: (
        <>
          <TetheringEditor tethering={draftTethering} onChange={setDraftTethering} />
          <ParityNote note={parityNote("tethering", "A hotspot rule", "A computer has no hotspot to govern.")} />
        </>
      ),
    },
  ];

  // Report unsaved changes upward, so the tab strip can dot a tab that isn't
  // showing and the save bar can name what is waiting. Joined to a string first:
  // a fresh array every render would loop the effect forever.
  const dirtyKey = sections
    .filter((s) => s.dirty)
    .map((s) => s.id)
    .join(",");
  useEffect(() => {
    onDirty(dirtyKey);
  }, [dirtyKey, onDirty]);

  const visible = sections.filter((s) => SECTION_TAB_OF[s.id] === section);
  const changedTitles = sections
    .filter((s) => s.dirty)
    .map((s) => SECTION_TITLE[s.id]);
  const firstDirty = sections.find((s) => s.dirty)?.id ?? null;
  // `dirty` can be true with nothing to name: splitting a control per device and
  // then un-splitting it leaves an empty `{deviceId: {}}` entry, which differs
  // from the saved map while every control's own overrides match. Rare, but the
  // bar must never read "0 changes:" — say the plain truth instead.
  const savePrompt =
    changedTitles.length === 0
      ? "Unsaved changes"
      : changedTitles.length === 1
        ? `Changed: ${changedTitles[0]}`
        : `${changedTitles.length} changes: ${changedTitles.join(", ")}`;

  return (
    <div className={dirty ? "has-save-bar" : undefined}>
      {section === "time" && (
        // Says what the rules actually cover, in the family's nouns — a phone is
        // a computer to us and not to a parent (see domain/deviceWords). Lives
        // on Time, the tab it is about, rather than as permanent page furniture.
        <p className="card-sub" style={{ margin: "12px 4px 12px" }}>
          {deviceNote} The schedule and time limit cover everything on it — apps,
          games, and the web alike.
        </p>
      )}

      <SectionList>
        {visible.map((s) => (
          <Section
            key={s.id}
            title={SECTION_TITLE[s.id]}
            summary={s.summary}
            changed={s.dirty}
            open={openSection === s.id}
            onToggle={() => onOpenSection(openSection === s.id ? null : s.id)}
          >
            {s.body}
          </Section>
        ))}
      </SectionList>

      {scheduleError && (
        <div style={{ marginTop: 14 }}>
          <Banner tone="warn">{scheduleError}</Banner>
        </div>
      )}

      {saveError && !scheduleError && (
        <div style={{ marginTop: 14 }}>
          <Banner tone="warn">{saveError}</Banner>
        </div>
      )}

      {willConfirm && !scheduleError && (
        <div style={{ marginTop: 14 }}>
          <Banner tone="info">
            You’ll confirm these changes
            {state.signer.label ? ` with ${state.signer.label}` : ""} before they
            apply.
          </Banner>
        </div>
      )}

      {/* One Save for every dimension, so it cannot live inside any one tab: a
          change on Time and a change on Safety are signed together, in a single
          clause with a shared issuedAt, or not at all. Fixed above the tab bar
          and only present when there is something to save. */}
      {dirty && (
        <div className="save-bar">
          <button
            type="button"
            className="save-bar-what"
            onClick={() => firstDirty && onOpenSection(firstDirty)}
            aria-label={`Show unsaved changes — ${savePrompt}`}
          >
            {savePrompt}
          </button>
          <Button variant="ghost" onClick={discard} disabled={saving}>
            Discard
          </Button>
          <Button
            variant="primary"
            onClick={save}
            disabled={saving || scheduleError != null}
          >
            {saving ? "Saving…" : "Save"}
          </Button>
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// App-scope policy card — per-app block + allowed-hours (enforced on-device)
// ---------------------------------------------------------------------------

function AppLimitCard({ child, policy }: { child: Child; policy: Policy }) {
  const { saveAppRule, removePolicy, state } = useCharter();
  const label = policy.scope.kind === "app" ? policy.scope.label : "App";

  // Saved snapshots (normalized) — the baseline the drafts diff against.
  const savedBlocked = policy.blocked ?? false;
  const savedScheduleOn = policy.schedule != null;
  const savedSchedule = useMemo(
    () => buildSchedule(policy.schedule),
    [policy.schedule],
  );

  const [draftBlocked, setDraftBlocked] = useState<boolean>(savedBlocked);
  const [scheduleOn, setScheduleOn] = useState<boolean>(savedScheduleOn);
  const [draftSchedule, setDraftSchedule] = useState<Schedule>(savedSchedule);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // Re-sync the drafts if the saved policy changes underneath us.
  useEffect(() => {
    setDraftBlocked(savedBlocked);
    setScheduleOn(savedScheduleOn);
    setDraftSchedule(savedSchedule);
    setSaveError(null);
  }, [savedBlocked, savedScheduleOn, savedSchedule]);

  // A per-app schedule is purely window-membership ("usable only in these
  // hours"); OFF (no schedule) means no per-app time limit at all.
  const effSaved = savedScheduleOn ? JSON.stringify(savedSchedule) : null;
  const effDraft = scheduleOn ? JSON.stringify(draftSchedule) : null;
  const dirty = draftBlocked !== savedBlocked || effSaved !== effDraft;
  // Same overnight-window guard as the device editor — block a save that would
  // otherwise throw at sign time (or store an unreachable window).
  const scheduleError = scheduleOn ? scheduleWindowError(draftSchedule) : null;

  async function save() {
    if (scheduleError) {
      setSaveError(scheduleError);
      return;
    }
    setSaveError(null);
    setSaving(true);
    try {
      // Block + allowed-hours travel together as ONE signed appRules change.
      await saveAppRule(child.id, policy.id, {
        blocked: draftBlocked,
        schedule: scheduleOn ? draftSchedule : undefined,
      });
    } catch (e) {
      // A signer failure (bunker timeout, rejected sign, wire validation) must
      // NOT vanish — the parent would think the change applied when it didn't.
      setSaveError(
        e instanceof Error && e.message
          ? `Couldn’t save these changes: ${e.message}`
          : "Couldn’t save these changes. Please try again.",
      );
    } finally {
      setSaving(false);
    }
  }

  const willConfirm =
    state.signer.connected && !state.signer.autoSign && dirty;

  return (
    <Card tinted>
      <div className="row-between">
        <h2 className="card-title" style={{ margin: 0 }}>
          {label}
        </h2>
        <Pill tone={draftBlocked ? "blocked" : "ok"}>
          {draftBlocked ? "Blocked" : "Allowed"}
        </Pill>
      </div>
      <p className="card-sub" style={{ marginTop: 4 }}>
        Block {label} from opening, or limit it to set hours — on top of the
        whole-computer rules. Kintrinsic enforces this on the device and never
        reports what was played.
      </p>

      {/* Block — a blocked app never opens, even during allowed screen time. */}
      <div className="row-between" style={{ marginTop: 14 }}>
        <div className="row-main">
          <span className="row-title">Block {label}</span>
          <br />
          <span className="row-sub">
            Stops it from opening on the device, even during allowed time.
          </span>
        </div>
        <Toggle
          checked={draftBlocked}
          label={`Block ${label}`}
          onChange={setDraftBlocked}
        />
      </div>

      {/* Allowed hours — the app is usable only inside these windows. */}
      <div
        className="row-between"
        style={{
          marginTop: 14,
          borderTop: "1px solid var(--line)",
          paddingTop: 14,
          opacity: draftBlocked ? 0.5 : 1,
        }}
      >
        <div className="row-main">
          <span className="row-title">Only allow during set hours</span>
          <br />
          <span className="row-sub">
            Outside these times, {label} won’t open.
          </span>
        </div>
        <Toggle
          checked={scheduleOn}
          label={`Limit ${label} to set hours`}
          onChange={setScheduleOn}
        />
      </div>
      {scheduleOn && (
        <div style={{ marginTop: 12, opacity: draftBlocked ? 0.5 : 1 }}>
          <ScheduleEditor schedule={draftSchedule} onChange={setDraftSchedule} />
        </div>
      )}

      {scheduleError && (
        <div style={{ marginTop: 14 }}>
          <Banner tone="warn">{scheduleError}</Banner>
        </div>
      )}

      {saveError && !scheduleError && (
        <div style={{ marginTop: 14 }}>
          <Banner tone="warn">{saveError}</Banner>
        </div>
      )}

      {willConfirm && !scheduleError && (
        <div style={{ marginTop: 14 }}>
          <Banner tone="info">
            You’ll confirm these changes
            {state.signer.label ? ` with ${state.signer.label}` : ""} before they
            apply.
          </Banner>
        </div>
      )}

      <div className="row-between" style={{ marginTop: 16, gap: 10 }}>
        <Button
          variant="ghost"
          disabled={saving}
          onClick={() => removePolicy(child.id, policy.id)}
        >
          Remove
        </Button>
        <Button
          variant="primary"
          disabled={!dirty || saving || scheduleError != null}
          onClick={save}
        >
          {saving ? "Saving…" : "Save limit"}
        </Button>
      </div>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Screen
// ---------------------------------------------------------------------------

export default function Limits() {
  const { state, addAppLimit, deviceStatus } = useCharter();
  const { children } = state;

  // `#/limits/<childId>` — set by Home, Family, Activity and Approvals so
  // tapping a ward anywhere opens THEIR rules, and the browser's back button
  // returns to the ward you came from.
  const [selectedId, selectWard] = useWardRoute(
    "limits",
    (id) => children.some((c) => c.id === id),
    children[0]?.id ?? "",
  );
  const [section, setSection] = useState<SectionTab>("time");
  // One row open at a time — the point of collapsing was to be able to read the
  // whole shape of a ward's rules at a glance, which a screen with five open
  // editors is back to not being.
  const [openSection, setOpenSection] = useState<string | null>(null);
  const [dirtyKey, setDirtyKey] = useState("");
  const dirtyIds = dirtyKey ? dirtyKey.split(",") : [];

  // Keep the selection valid as the family changes.
  const selected = children.find((c) => c.id === selectedId) ?? children[0];

  /** Open a row, switching to its tab first — the save bar names changes that
   *  may be sitting on a tab you can't currently see. */
  const openAt = (id: string | null) => {
    if (id) setSection(tabOfSection(id));
    setOpenSection(id);
  };

  if (children.length === 0) {
    return (
      <EmptyState emoji="🗓️" title="No limits yet">
        Add a child in Family first, then set their daily schedule and time
        limit here.
      </EmptyState>
    );
  }

  const devicePolicy = selected.policies.find((p) => p.scope.kind === "device");
  const appPolicies = selected.policies.filter((p) => p.scope.kind === "app");
  const freshest = freshestStatusFor(selected, deviceStatus, Date.now());
  const status = liveStatusFor(selected, freshest, Date.now());
  const hasLiveDevice = selected.devices.some(
    (d) => d.pairing === "paired" && d.devicePubkey && /^[0-9a-f]{64}$/.test(d.devicePubkey),
  );

  /** Switching ward drops the drafts (the editor is keyed by child), so say so
   *  rather than discarding a parent's unsaved rules silently. */
  const switchWard = (id: string) => {
    if (
      dirtyKey &&
      !window.confirm(
        `You have unsaved changes to ${selected.name}'s rules. Leave them unsaved?`,
      )
    )
      return;
    setOpenSection(null);
    selectWard(id);
  };

  return (
    <>
      {/* Whose rules, then which rules. Two levels, drawn differently on
          purpose: a filled switcher over an underline strip reads as a
          hierarchy where two identical strips would read as a double row. */}
      <div className="sticky-ward-switcher">
        <WardTabs
          wards={children}
          selected={selected.id}
          onSelect={switchWard}
          label="Whose rules"
        />
        {/* Only when there is no switcher to say it. A one-ward family gets the
            name and avatar; a two-ward family already has "Sam" highlighted in
            the switcher, and repeating it thirty pixels below is the exact
            duplication this pass exists to remove. The status pill is NOT
            duplication — the switcher can't say whether the rules are biting
            right now — so it rides the tab strip instead, costing no height. */}
        {children.length < 2 && (
          <WardHeading child={selected} as="div" />
        )}
        {devicePolicy && (
          <SubTabs
            label="Which rules"
            value={section}
            options={SECTION_TABS.map((t) => ({
              id: t.id,
              label: t.label,
              count: dirtyIds.filter((d) => tabOfSection(d) === t.id).length,
            }))}
            onChange={setSection}
            trailing={
              <Pill tone={status.allowedNow ? "ok" : "blocked"}>
                {status.allowedNow ? "Allowed now" : "Locked"}
              </Pill>
            }
          />
        )}
      </div>

      {/* Honesty at the point of action: signed limits reach PAIRED devices
          within seconds; without one there is nothing to enforce them yet. Was
          two banners saying the same thing in different words — and neither of
          them offered the way to fix it. */}
      {!hasLiveDevice && (
        <div style={{ margin: "12px 0" }}>
          <Banner tone="info">
            <span>
              Nothing is enforcing these yet — they start working the moment{" "}
              {selected.name}'s phone or computer is set up.{" "}
              <button
                type="button"
                className="btn btn-ghost"
                style={{ minHeight: "auto", padding: "0 2px" }}
                onClick={() => {
                  window.location.hash = "/family";
                }}
              >
                Set up a device
              </button>
            </span>
          </Banner>
        </div>
      )}

      {devicePolicy ? (
        <DeviceLimits
          key={selected.id}
          child={selected}
          policy={devicePolicy}
          section={section}
          onDirty={setDirtyKey}
          openSection={openSection}
          onOpenSection={openAt}
        />
      ) : (
        // Unreachable for a ward added in this app — `addChild` mints a
        // device-scope policy — but a restored backup from an older household
        // could arrive without one, and a blank screen would be the app quietly
        // showing nothing where a charter should be.
        <Banner tone="warn">
          {selected.name} has no charter to edit yet. Setting up a device
          creates one.
        </Banner>
      )}

      {/* Per-app rules sit on Content, beside the app allow/block list they
          build on — not in a fourth place further down the same long page. */}
      {devicePolicy && section === "content" && (
        <>
          <SectionLabel>Single out a game or app</SectionLabel>
          <p className="card-sub" style={{ margin: "0 4px 10px" }}>
            On top of the rules above — block one outright, or hold it to set
            hours.
          </p>
          {appPolicies.length > 0 && (
            <div className="stack">
              {appPolicies.map((p) => (
                <AppLimitCard key={p.id} child={selected} policy={p} />
              ))}
            </div>
          )}
          <div style={{ marginTop: 16 }}>
            <AddAppLimit onAdd={(app) => addAppLimit(selected.id, app)} />
          </div>
        </>
      )}
    </>
  );
}
