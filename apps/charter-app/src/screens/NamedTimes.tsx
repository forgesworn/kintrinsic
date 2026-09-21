// The Named times editor — ONE guardian surface replacing the separate
// Learning and App-time-limits (buckets) sections (2026-08 spec). A named
// time is a set of apps sharing one policy: Free (never counted, like the old
// Learning section), Counted (a shared daily/weekly allowance, like the old
// buckets), or On request (blocked, but askable).
//
// Limits.tsx owns the section registry and the drafts this reads/writes
// (`domain/namedTimes.ts` compiles/decompiles them); this file is only the
// editing surface, kept out of Limits.tsx because that screen is already
// unwieldy.

import { useId, useState } from "react";
import {
  isDeviceShaped,
  MAX_BUCKETS,
  moveApp,
  namedTimeSlug,
  NAMED_TIME_SUGGESTIONS,
  partitionOnPolicyChange,
  type GroupPolicy,
  type NamedGroup,
} from "../domain/namedTimes";
import type { LearningAppSel } from "../domain/types";
import { LEARNING_CATALOGUE } from "../data/learning_catalogue";
import { Banner, Button, Card } from "../components/ui";
import { AddSiteForm, Segmented, Stepper, Toggle } from "./Limits";
import type { AppSection, ReportedApp } from "../domain/deviceApps";
import {
  attachAllSignatures,
  humanLabelFor,
  isCmdlineIdentity,
  shouldAttachSignature,
  signatureFor,
  stripAllSignatures,
  withSignature,
  withoutSignature,
} from "../domain/launchSignatures";

const POLICY_OPTIONS: { value: GroupPolicy; label: string }[] = [
  { value: "free", label: "Free" },
  { value: "counted", label: "Counted" },
  { value: "onRequest", label: "On request" },
];

const POLICY_HINT: Record<GroupPolicy, string> = {
  free: "Unlimited — this time is never counted, the same as homework always has been.",
  counted:
    "A shared daily and/or weekly allowance. When it runs out these apps close for the period; everything else keeps working.",
  onRequest: "Blocked — but they can ask, and you can say yes just for now.",
};

function hasAnyOf(groups: NamedGroup[], policy: GroupPolicy): boolean {
  return groups.some((g) => g.policy === policy);
}

/** Applies a per-app filter within every section without disturbing which
 *  devices are shown or their order — used to keep the free-policy
 *  device-shaped guard (see the `AppPicker` call below) working section by
 *  section instead of on one flat list. */
function filterSections(
  sections: AppSection[],
  predicate: (a: ReportedApp) => boolean,
): AppSection[] {
  return sections.map((s) => ({ ...s, apps: s.apps.filter(predicate) }));
}

/**
 * A whole-axis master switch — mirrors the old Learning/buckets sections'
 * own "Learning time" / "Use time buckets" toggle. Off pauses every group of
 * that policy without deleting a single one of them (see the compile doc in
 * `domain/namedTimes.ts`): the fix for a review finding that turning this off
 * used to be indistinguishable from having no groups at all, which snapped
 * back on — silently — the moment anything else was saved.
 */
function AxisToggle({
  label,
  enabled,
  onChange,
  pausedHint,
}: {
  label: string;
  enabled: boolean;
  onChange: (on: boolean) => void;
  pausedHint: string;
}) {
  return (
    <div className="row-between" style={{ borderTop: "1px solid var(--line)", paddingTop: 10 }}>
      <div className="row-main">
        <span className="row-title">
          {label} are {enabled ? "active" : "paused"}
        </span>
        <br />
        <span className="row-sub">{enabled ? "Enforcing normally." : pausedHint}</span>
      </div>
      <Toggle checked={enabled} label={`${label} active`} onChange={onChange} />
    </div>
  );
}

/** The gentle empty-state line for a device with no reported inventory — the
 *  same copy this picker has always shown when there was nothing to offer,
 *  now anchored under the device it's actually true of. */
function DeviceNotReportedYet() {
  return (
    <p className="card-sub" style={{ margin: 0 }}>
      This device hasn’t reported its apps yet. Open it once while it’s
      online, or type the identifier below.
    </p>
  );
}

/**
 * A group's app membership, shown as remove-able pills plus a picker over
 * what the device reported (or a manually-typed identifier for something not
 * yet reported). The SAME picker for every policy — a counted, free or
 * on-request group all pick native apps the identical way; a free group adds
 * the websites block underneath (see `FreeSites`).
 *
 * The offered apps are grouped one section per paired device (see
 * `domain/deviceApps.appsBySection`) — a flat merged list mixed a laptop's
 * flatpak ids in with a phone's Android packages as though they were one
 * vocabulary, when picking the wrong one writes a rule that can never once
 * fire on the device it was meant for.
 */
function AppPicker({
  apps,
  sections,
  labelFor,
  onAdd,
  onRemove,
  wardName,
}: {
  apps: string[];
  sections: AppSection[];
  labelFor: (pkg: string) => string;
  onAdd: (pkg: string) => void;
  onRemove: (pkg: string) => void;
  /** For the "from <ward>'s account" mark on a row from the ward's own
   *  writable inventory area — see `AppRef.userInstalled`. */
  wardName: string;
}) {
  const [adding, setAdding] = useState(false);
  const [custom, setCustom] = useState("");
  const [customError, setCustomError] = useState<string | null>(null);
  const customId = useId();

  return (
    <div>
      {apps.length === 0 && (
        <p className="card-sub" style={{ marginTop: 0 }}>
          No apps yet — this named time does nothing until you add one.
        </p>
      )}
      <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
        {apps.map((pkg) => (
          <button
            key={pkg}
            type="button"
            className="pill pill-neutral pill-action"
            onClick={() => onRemove(pkg)}
          >
            {labelFor(pkg)} ×
          </button>
        ))}
      </div>

      {adding ? (
        <div className="stack" style={{ marginTop: 10 }}>
          {sections.length === 0 ? (
            <DeviceNotReportedYet />
          ) : (
            sections.map((s) => {
              const offered = s.apps.filter((a) => !apps.includes(a.pkg));
              return (
                <div key={s.deviceId} style={{ marginBottom: 10 }}>
                  <div className="row-sub" style={{ marginBottom: 4 }}>
                    {s.platform === "android" ? "📱" : "💻"} {s.label}
                  </div>
                  {s.apps.length === 0 ? (
                    <DeviceNotReportedYet />
                  ) : (
                    <div style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
                      {offered.map((a) => (
                        <button
                          key={a.pkg}
                          type="button"
                          className="pill pill-neutral pill-action"
                          onClick={() => {
                            onAdd(a.pkg);
                            setAdding(false);
                          }}
                        >
                          + {a.label}
                          {/* A ward-writable inventory entry — flagged so
                              it's never mistaken for something the ward has
                              no way to have altered. Says WHERE it lives
                              (F3 review: "installed by" claims agency the
                              flag doesn't carry — contract.md's userInstalled
                              explicitly covers a PARENT running `flatpak
                              install --user` FOR their child too), never who
                              put it there. */}
                          {a.userInstalled ? ` (from ${wardName}'s account)` : ""}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              );
            })
          )}
          <div className="field">
            <label className="field-label" htmlFor={customId}>
              Or its identifier (e.g. a flatpak id like{" "}
              <code>com.mojang.Minecraft</code>, or the program name)
            </label>
            <div style={{ display: "flex", gap: 8 }}>
              <input
                id={customId}
                className="input"
                placeholder="com.mojang.Minecraft"
                value={custom}
                onChange={(e) => {
                  setCustom(e.target.value);
                  if (customError) setCustomError(null);
                }}
              />
              <Button
                variant="primary"
                disabled={custom.trim().length === 0}
                onClick={() => {
                  const v = custom.trim();
                  if (!v) return;
                  // SAFETY (binding, carried from the charterd review): a
                  // `cmdline:` identity may only ever enter a saved policy
                  // through the curated launch-signatures table — never as
                  // free text. The device's own length floor bounds it to
                  // 8+ chars, not to a SAFE 8+ chars: `cmdline:/usr/lib`
                  // matches 85 running processes, measured live.
                  if (isCmdlineIdentity(v)) {
                    setCustomError(
                      "Kintrinsic attaches this kind of identity for supported apps itself (like Minecraft) — you can't type it directly.",
                    );
                    return;
                  }
                  onAdd(v);
                  setCustom("");
                  setAdding(false);
                }}
              >
                Add
              </Button>
            </div>
            {customError && (
              <p className="card-sub" style={{ color: "var(--warn, #9a6516)", marginTop: 4 }}>
                {customError}
              </p>
            )}
          </div>
          <Button variant="ghost" block onClick={() => setAdding(false)}>
            Cancel
          </Button>
        </div>
      ) : (
        <Button variant="secondary" block onClick={() => setAdding(true)}>
          + Add an app
        </Button>
      )}
    </div>
  );
}

/** The two counted axes — per-day and per-week, 15-minute steps, each
 *  independently clearable. At least one must stay on to save (enforced by
 *  `namedTimesError`, surfaced as a banner by the caller). */
function CountedAxes({
  group,
  onChange,
}: {
  group: NamedGroup;
  onChange: (patch: Partial<NamedGroup>) => void;
}) {
  const dailyOn = group.dailyMinutes != null;
  const weeklyOn = group.weeklyMinutes != null;
  return (
    <div className="stack" style={{ marginTop: 12 }}>
      <div className="row-between">
        <span className="row-sub">Per day</span>
        <Toggle
          checked={dailyOn}
          label="Per-day limit"
          onChange={(on) => onChange({ dailyMinutes: on ? 60 : undefined })}
        />
      </div>
      {dailyOn && (
        <Stepper
          label="Per-day limit"
          value={group.dailyMinutes ?? 60}
          min={15}
          max={1440}
          step={15}
          onChange={(v) => onChange({ dailyMinutes: v })}
        />
      )}
      <div className="row-between">
        <span className="row-sub">Per week</span>
        <Toggle
          checked={weeklyOn}
          label="Per-week limit"
          onChange={(on) => onChange({ weeklyMinutes: on ? 300 : undefined })}
        />
      </div>
      {weeklyOn && (
        <Stepper
          label="Per-week limit"
          value={group.weeklyMinutes ?? 300}
          min={15}
          max={10080}
          step={15}
          onChange={(v) => onChange({ weeklyMinutes: v })}
        />
      )}
    </div>
  );
}

/**
 * The websites block for a Free-policy group only: the curated catalogue
 * (verified domain pins — see `data/learning_catalogue`) plus a custom "Add
 * another site" form. Ported verbatim from the old Learning section; the one
 * thing dropped in this pass is "their own projects" (vouching for a home-
 * built script by absolute path) — a genuinely advanced edge case, and a
 * documented follow-up rather than a silent loss.
 */
function FreeSites({
  appIds,
  onToggle,
  learningApps,
  onAddLearningApp,
  runtimeMissingOn,
}: {
  appIds: string[];
  onToggle: (app: LearningAppSel, on: boolean) => void;
  /** Every site's full detail known this session — the catalogue plus
   *  whatever custom sites have been added (this session or previously). */
  learningApps: LearningAppSel[];
  onAddLearningApp: (app: LearningAppSel) => void;
  runtimeMissingOn: string[];
}) {
  const has = (id: string) => appIds.includes(id);
  const customSites = learningApps.filter(
    (a) => a.kind === "site" && has(a.id) && !LEARNING_CATALOGUE.some((c) => c.id === a.id),
  );
  const anySiteOn =
    appIds.some((id) => LEARNING_CATALOGUE.some((c) => c.id === id)) || customSites.length > 0;

  return (
    <div style={{ marginTop: 12 }}>
      <div className="field-label">Learning websites</div>
      <p className="card-sub" style={{ margin: "2px 0 8px" }}>
        Each appears on their computer as its own app, locked to that site —
        outside links simply don’t load in it.
      </p>
      {anySiteOn && (
        <p className="card-sub" style={{ margin: "2px 0 8px" }}>
          Firefox stays their browser. While these are on, Chromium opens only
          these sites on their computer.
        </p>
      )}
      {runtimeMissingOn.length > 0 && (
        <Banner tone="warn">
          {runtimeMissingOn.join(" and ")}{" "}
          {runtimeMissingOn.length > 1 ? "don’t" : "doesn’t"} have Chromium
          installed, so these sites can’t open there yet. Install it and
          they’ll appear on their own — nothing to change here.
        </Banner>
      )}
      <div className="stack" style={{ gap: 2 }}>
        {LEARNING_CATALOGUE.map((c) => (
          <div className="row-between" key={c.id} style={{ padding: "4px 0" }}>
            <span>{c.label}</span>
            <Toggle
              checked={has(c.id)}
              label={`learning: ${c.label}`}
              onChange={(on) => onToggle(c, on)}
            />
          </div>
        ))}
        {customSites.map((c) => (
          <div className="row-between" key={c.id} style={{ padding: "4px 0" }}>
            <span>
              {c.label}
              <span className="muted" style={{ fontSize: "var(--fs-small)", marginLeft: 8 }}>
                {c.domains?.[0]}
              </span>
            </span>
            <Button variant="ghost" onClick={() => onToggle(c, false)}>
              Remove
            </Button>
          </div>
        ))}
      </div>
      <AddSiteForm
        takenIds={learningApps.map((a) => a.id)}
        onAdd={(app) => {
          onAddLearningApp(app);
          onToggle(app, true);
        }}
      />
    </div>
  );
}

export function NamedTimesSection({
  groups,
  onChange,
  available,
  sections,
  learningApps,
  onAddLearningApp,
  runtimeMissingOn,
  capMinutes,
  onCapMinutesChange,
  learningEnabled,
  onLearningEnabledChange,
  bucketsEnabled,
  onBucketsEnabledChange,
  error,
  wardName,
  cmdlineOk,
  cmdlineParityNote,
}: {
  groups: NamedGroup[];
  onChange: (groups: NamedGroup[]) => void;
  /** Apps the device reported — the native-app picker source (merged across
   *  devices; named times, like buckets before it, is never split). */
  available: ReportedApp[];
  /** The SAME inventory as `available`, grouped one section per paired
   *  device — what the picker actually renders, so a guardian can see which
   *  device an app lives on even though the rule isn't split. */
  sections: AppSection[];
  /** Full LearningAppSel detail known this session for every site ANY free
   *  group references — see `FreeSites`. */
  learningApps: LearningAppSel[];
  onAddLearningApp: (app: LearningAppSel) => void;
  runtimeMissingOn: string[];
  /** The advanced, shared cap on ALL free time (not per-group — see the
   *  compile doc in `domain/namedTimes.ts`). */
  capMinutes?: number;
  onCapMinutesChange: (m: number | undefined) => void;
  /** The two whole-axis pause switches — mirror the old Learning/buckets
   *  sections' own master toggle. Off does NOT delete anything: every
   *  group's configuration survives, only enforcement pauses. See the
   *  compile doc in `domain/namedTimes.ts`. */
  learningEnabled: boolean;
  onLearningEnabledChange: (on: boolean) => void;
  bucketsEnabled: boolean;
  onBucketsEnabledChange: (on: boolean) => void;
  error: string | null;
  /** The ward's own name — for the "from <ward>'s account" mark on a
   *  user-installed row (see `AppPicker`). */
  wardName: string;
  /** Whether EVERY one of the ward's devices supports the `cmdline:`
   *  identity form (`wardenSupport`'s `cmdlineIdentity`, linux >= 705,
   *  android NEVER). Picking a launch-signature app (Minecraft) only
   *  attaches its `cmdline:` identity when this is true — an old warden
   *  treats an unmatched `cmdline:` string as inert, which loosens a
   *  BLOCKED list, so attaching it there would be the wrong direction to
   *  fail in. */
  cmdlineOk: boolean;
  /** The parity sentence to show instead, when `cmdlineOk` is false and the
   *  guardian just picked an app with a launch signature — built by the
   *  caller (`unsupportedNote`) so this file doesn't need `wardenSupport`. */
  cmdlineParityNote: string | null;
}) {
  const [creating, setCreating] = useState(false);
  const [draftLabel, setDraftLabel] = useState("");
  const [moveNotice, setMoveNotice] = useState<{
    pkg: string;
    appLabel: string;
    fromId: string;
    fromLabel: string;
  } | null>(null);
  const [droppedNotice, setDroppedNotice] = useState<{ groupLabel: string; names: string[] } | null>(null);
  // Set only right after picking an app with a launch signature (Minecraft) —
  // cleared by any OTHER add, so it always reflects the most recent pick
  // rather than lingering from an earlier one.
  const [signatureNotice, setSignatureNotice] = useState<{ label: string; why: string } | null>(null);
  const [signatureGap, setSignatureGap] = useState<string | null>(null);
  // Set only right after a policy switch strips a `cmdline:` identity that
  // no longer belongs where the group is going (see `setPolicy`).
  const [signatureStrippedNotice, setSignatureStrippedNotice] = useState<string | null>(null);

  const labelFor = (pkg: string) =>
    humanLabelFor(pkg) ??
    available.find((a) => a.pkg === pkg)?.label ??
    learningApps.find((a) => a.id === pkg)?.label ??
    pkg;
  const groupLabel = (id: string) => groups.find((g) => g.id === id)?.label ?? id;

  const put = (id: string, patch: Partial<NamedGroup>) =>
    onChange(groups.map((g) => (g.id === id ? { ...g, ...patch } : g)));
  const remove = (id: string) => {
    onChange(groups.filter((g) => g.id !== id));
    setMoveNotice((n) => (n && n.fromId === id ? null : n));
  };

  /** Add (or MOVE — see `domain/namedTimes.moveApp`) an app into a group. An
   *  app already living elsewhere is never duplicated: it's pulled out of its
   *  old group, and an inline "moved from X" note offers an undo.
   *
   *  A launch signature (Minecraft) rides along, but ONLY into a COUNTED
   *  group (review finding, 2026-08-03) — the other two policies are
   *  actively harmful, not merely pointless, to attach it into:
   *    - `onRequest` compiles into BOTH `askFirst` and `blocked`, so the raw
   *      needle would render on the ward's OWN tray ("Ask to open
   *      cmdline:net.minecraft.client.main.Main" — an internal string on a
   *      child's screen), and a hold is per-pkg exact: approving "open
   *      Minecraft" would lift only the launcher while the `cmdline:` entry
   *      stayed blocked, so the sweep kills the JVM anyway — the guardian
   *      taps yes, the log says yes, the game dies regardless.
   *    - `free` compiles into `learning`, and a `cmdline:` identity there
   *      grants FREE TIME whenever the exe is root-owned — while argv (all a
   *      `cmdline:` match can ever see) stays ward-written regardless of who
   *      owns the exe. Filing Minecraft as Free would hand the ward a
   *      screen-budget off-switch they can type themselves
   *      (`xterm -T net.minecraft.client.main.Main`), with the needle
   *      readable straight off the ward's own transparency surface.
   *
   *  When `cmdlineOk`, the identity is attached to the SAME group `pkg`
   *  landed in, and any stale copy left behind in the group it moved FROM is
   *  cleaned up (`withoutSignature` only drops it once nothing there still
   *  needs it — two launchers of the same game can share one group). When
   *  the ward's devices don't all support the form, nothing is attached and
   *  the parity note is surfaced instead — the whole point of gating it.
   *  Switching a counted group carrying the identity to Free/On-request
   *  later strips it too — see `setPolicy`. */
  const addApp = (groupId: string, pkg: string) => {
    const result = moveApp(groups, pkg, groupId);
    let nextGroups = result.groups;
    const sig = signatureFor(pkg);
    const targetPolicy = groups.find((g) => g.id === groupId)?.policy;
    const targetIsCounted = targetPolicy === "counted";
    if (sig && targetPolicy && shouldAttachSignature(targetPolicy, cmdlineOk)) {
      nextGroups = nextGroups.map((g) =>
        g.id === groupId ? { ...g, apps: withSignature(g.apps, pkg) } : g,
      );
      setSignatureNotice({ label: sig.label, why: sig.why });
      setSignatureGap(null);
    } else if (sig && targetIsCounted) {
      // Counted, but this ward's devices don't all support the form yet —
      // the parity gap, not the policy gap.
      setSignatureNotice(null);
      setSignatureGap(sig.label);
    } else {
      // Either no signature matched this pkg, or the group's policy
      // (Free/On-request) is one `shouldAttachSignature` never allows —
      // silent, since attaching here was never on offer to begin with.
      setSignatureNotice(null);
      setSignatureGap(null);
    }
    if (sig && result.movedFrom) {
      nextGroups = nextGroups.map((g) =>
        g.id === result.movedFrom ? { ...g, apps: withoutSignature(g.apps, pkg) } : g,
      );
    }
    onChange(nextGroups);
    setMoveNotice(
      result.movedFrom
        ? { pkg, appLabel: labelFor(pkg), fromId: result.movedFrom, fromLabel: groupLabel(result.movedFrom) }
        : null,
    );
  };
  const removeApp = (groupId: string, pkg: string) => {
    const remaining = (groups.find((g) => g.id === groupId)?.apps ?? []).filter((p) => p !== pkg);
    // Drop the orphaned `cmdline:` identity too, unless another app still in
    // this group is the same launch signature (see `withoutSignature`).
    put(groupId, { apps: withoutSignature(remaining, pkg) });
    // The app the banner is offering to undo no longer exists anywhere —
    // offering "undo" for it would put it right back after the guardian just
    // removed it on purpose.
    setMoveNotice((n) => (n && n.pkg === pkg ? null : n));
  };

  const undoMove = () => {
    if (!moveNotice) return;
    onChange(moveApp(groups, moveNotice.pkg, moveNotice.fromId).groups);
    setMoveNotice(null);
  };

  /** A group's policy switch — Free ↔ Counted/On-request. Leaving Free drops
   *  any learning-site id (a catalogue entry or guardian-added site): Counted
   *  and On-request only ever match a real device pkg, and keeping a site id
   *  there would be a rule that can never fire (the exact trap the old
   *  buckets editor's doc warned about). Named plainly rather than silently
   *  vanished — and it stays time-free only if it lives in ANOTHER free
   *  group (nothing here moves it there automatically).
   *
   *  Leaving Counted strips any `cmdline:` launch-signature identity the
   *  group is carrying (review finding, 2026-08-03) — that vocabulary only
   *  belongs in a Counted group (see `addApp`'s doc for why Free/On-request
   *  are actively harmful, not merely a no-op, to carry it into), so it must
   *  not silently ride along into either. The matched pkg itself (Minecraft
   *  Launcher, say) stays; only the derived identity is removed. */
  const setPolicy = (g: NamedGroup, v: GroupPolicy) => {
    const leavingFree = g.policy === "free" && v !== "free";
    const { kept, dropped } = leavingFree ? partitionOnPolicyChange(g.apps) : { kept: g.apps, dropped: [] };
    const leavingCounted = g.policy === "counted" && v !== "counted";
    // ENTERING Counted re-attaches any signature its own apps already
    // justify (review finding, New-3): round-tripping Counted → Free →
    // Counted used to leave the identity gone with no way back — the
    // guardian was told plainly when it was REMOVED (the strip notice
    // below) but never that it hadn't come back, so a Counted group could
    // quietly lose its safety net. Re-attaching on the way back in is the
    // SAME condition that would have attached it via a fresh pick
    // (`addApp`), so it's the honest default rather than a surprise.
    const enteringCounted = g.policy !== "counted" && v === "counted";
    let finalApps = kept;
    if (leavingCounted) {
      finalApps = stripAllSignatures(kept);
    } else if (enteringCounted && cmdlineOk) {
      finalApps = attachAllSignatures(kept);
    }
    const strippedIdentity = leavingCounted && finalApps !== kept;
    const reenteringSig = enteringCounted ? kept.map(signatureFor).find((s) => s) : undefined;
    put(g.id, {
      policy: v,
      apps: finalApps,
      ...(v === "counted" && g.dailyMinutes == null && g.weeklyMinutes == null ? { dailyMinutes: 60 } : {}),
    });
    setDroppedNotice(
      dropped.length > 0
        ? { groupLabel: g.label || "this named time", names: dropped.map((id) => labelFor(id)) }
        : null,
    );
    setSignatureStrippedNotice(
      strippedIdentity
        ? `Minecraft's extra identity only works in a Counted named time, so it's been removed from "${g.label || "this named time"}".`
        : null,
    );
    if (reenteringSig) {
      if (cmdlineOk) {
        setSignatureNotice({ label: reenteringSig.label, why: reenteringSig.why });
        setSignatureGap(null);
      } else {
        setSignatureNotice(null);
        setSignatureGap(reenteringSig.label);
      }
    }
  };

  const createGroup = (label: string) => {
    const name = label.trim();
    if (!name) return;
    const id = namedTimeSlug(name, groups.map((g) => g.id));
    onChange([...groups, { id, label: name, apps: [], policy: "counted", dailyMinutes: 60 }]);
    setDraftLabel("");
    setCreating(false);
  };

  const hasFree = groups.some((g) => g.policy === "free");
  const countedCount = groups.filter((g) => g.policy === "counted").length;

  return (
    <div className="stack">
      <p className="card-sub" style={{ marginTop: 0 }}>
        Give a set of apps a name, then decide what that name means: Free
        (never counted, like homework), Counted (a shared daily or weekly
        allowance), or On request (blocked, but they can ask). Each app
        belongs to one named time at a time — picking it for another one
        moves it there.
      </p>

      {error && <Banner tone="warn">{error}</Banner>}

      {moveNotice && (
        <Banner tone="info">
          Moved {moveNotice.appLabel} from {moveNotice.fromLabel}.{" "}
          <Button variant="ghost" onClick={undoMove}>
            Undo
          </Button>
        </Banner>
      )}

      {droppedNotice && (
        <Banner tone="warn">
          {droppedNotice.names.join(", ")} {droppedNotice.names.length > 1 ? "are learning sites" : "is a learning site"},
          not apps on their device — {droppedNotice.names.length > 1 ? "they" : "it"} can’t be in a Counted or On-request
          time, so {droppedNotice.names.length > 1 ? "they’ve" : "it’s"} been taken out of “{droppedNotice.groupLabel}”.
          Add {droppedNotice.names.length > 1 ? "them" : "it"} to a Free named time to keep{" "}
          {droppedNotice.names.length > 1 ? "them" : "it"} time-free.
        </Banner>
      )}

      {/* A launch signature just attached (design §2.5) — a technical launch
          fact, not a judgement about what the app is for. */}
      {signatureNotice && (
        <Banner tone="info">
          {signatureNotice.why}
        </Banner>
      )}

      {/* The ward's devices don't ALL support the `cmdline:` form yet, so
          nothing was attached — an old warden would treat the string as
          inert, which loosens a blocked list rather than tightening it. */}
      {signatureGap && cmdlineParityNote && <Banner tone="warn">{cmdlineParityNote}</Banner>}

      {/* A policy switch just took a group's `cmdline:` identity out from
          under it (design §2.5's Counted-only restriction, review finding). */}
      {signatureStrippedNotice && <Banner tone="info">{signatureStrippedNotice}</Banner>}

      {hasAnyOf(groups, "free") && (
        <AxisToggle
          label="Free times"
          enabled={learningEnabled}
          onChange={onLearningEnabledChange}
          pausedHint="Paused — free apps count as normal screen time until you turn this back on. Nothing here is deleted."
        />
      )}
      {hasAnyOf(groups, "counted") && (
        <AxisToggle
          label="Counted times"
          enabled={bucketsEnabled}
          onChange={onBucketsEnabledChange}
          pausedHint="Paused — counted apps are unlimited until you turn this back on. Nothing here is deleted."
        />
      )}

      <div className="stack">
        {groups.map((g) => (
          <Card key={g.id} tinted>
            <div className="row-between">
              <input
                className="input"
                value={g.label}
                aria-label="Named time"
                onChange={(e) => put(g.id, { label: e.target.value.slice(0, 32) })}
              />
              <Button variant="ghost" onClick={() => remove(g.id)}>
                Remove
              </Button>
            </div>

            <div style={{ marginTop: 10 }}>
              <Segmented<GroupPolicy>
                value={g.policy}
                options={POLICY_OPTIONS}
                onChange={(v) => setPolicy(g, v)}
              />
              <p className="card-sub" style={{ margin: "6px 0 0" }}>
                {POLICY_HINT[g.policy]}
              </p>
            </div>

            {g.policy === "counted" && <CountedAxes group={g} onChange={(patch) => put(g.id, patch)} />}

            <div className="field-label" style={{ marginTop: 12 }}>
              Apps in {g.label || "this named time"}
            </div>
            <AppPicker
              apps={g.apps}
              // A free group ALSO carries learning-site ids (khan-academy, a
              // custom site slug) which are never device-reported — offering
              // the raw `available` list there risks a guardian typing/
              // picking something that only LOOKS native. Reinstates the old
              // Learning section's own native-picker guard (device-shaped
              // filter), which this shared picker had dropped.
              sections={
                g.policy === "free"
                  ? filterSections(sections, (a) => isDeviceShaped(a.pkg))
                  : sections
              }
              labelFor={labelFor}
              onAdd={(pkg) => addApp(g.id, pkg)}
              onRemove={(pkg) => removeApp(g.id, pkg)}
              wardName={wardName}
            />

            {g.policy === "free" && (
              <FreeSites
                appIds={g.apps}
                onToggle={(app, on) => (on ? addApp(g.id, app.id) : removeApp(g.id, app.id))}
                learningApps={learningApps}
                onAddLearningApp={onAddLearningApp}
                runtimeMissingOn={runtimeMissingOn}
              />
            )}
          </Card>
        ))}
      </div>

      {creating ? (
        <Card>
          <div className="field-label">Name it</div>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 8, margin: "6px 0" }}>
            {NAMED_TIME_SUGGESTIONS.map((s) => (
              <button
                key={s}
                type="button"
                className="pill pill-neutral pill-action"
                onClick={() => setDraftLabel(s)}
              >
                {s}
              </button>
            ))}
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <input
              className="input"
              placeholder="Name this time (e.g. Play)"
              aria-label="New named time"
              value={draftLabel}
              onChange={(e) => setDraftLabel(e.target.value.slice(0, 32))}
              onKeyDown={(e) => e.key === "Enter" && createGroup(draftLabel)}
            />
            <Button variant="primary" disabled={!draftLabel.trim()} onClick={() => createGroup(draftLabel)}>
              Add
            </Button>
          </div>
          <Button
            variant="ghost"
            block
            onClick={() => {
              setCreating(false);
              setDraftLabel("");
            }}
          >
            Cancel
          </Button>
        </Card>
      ) : (
        <Button variant="secondary" block onClick={() => setCreating(true)}>
          + Named time
        </Button>
      )}

      {hasFree && (
        <div className="row-between" style={{ borderTop: "1px solid var(--line)", paddingTop: 14 }}>
          <span className="row-sub">Cap free time (advanced)</span>
          <Toggle
            checked={capMinutes != null}
            label="Cap free time"
            onChange={(on) => onCapMinutesChange(on ? 120 : undefined)}
          />
        </div>
      )}
      {hasFree && capMinutes != null && (
        <div style={{ paddingLeft: 2 }}>
          <Stepper
            label="Daily cap on free time"
            value={capMinutes}
            min={15}
            max={1440}
            step={15}
            onChange={onCapMinutesChange}
          />
          <p className="card-sub" style={{ margin: "6px 0 0" }}>
            Past the cap, free apps count as normal screen time (they are
            never locked).
          </p>
        </div>
      )}

      {countedCount > 0 && (
        <p className="card-sub" style={{ marginTop: 4 }}>
          {countedCount}/{MAX_BUCKETS} counted named times used.
        </p>
      )}
    </div>
  );
}
