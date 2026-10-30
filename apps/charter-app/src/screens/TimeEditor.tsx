import "./rule-editor.css";
import { useEffect, useMemo, useRef, useState } from "react";
import { Banner, Button, Card } from "../components/ui";
import { RuleSaveBar, useLeaveGuard } from "../components/RuleSaveBar";
import { LEARNING_CATALOGUE } from "../data/learning_catalogue";
import { appsBySection } from "../domain/deviceApps";
import { humanLabelFor } from "../domain/launchSignatures";
import {
  addNamedTime,
  canUseTimes,
  customTimeTemplate,
  learningTimeTemplate,
  namedTimeProblems,
  removeNamedTime,
  setTimeWindows,
  timeSlug,
  updateNamedTime,
  type TimeWindows,
} from "../domain/times";
import type { Child, Device, NamedTime, Policy, Schedule, Weekday } from "../domain/types";
import { WEEKDAYS } from "../domain/types";
import { wardSettingsPath } from "../domain/wardRoute";
import { siteFault } from "../wire/times";
import type { LearningApp, TimePosture } from "../wire/types";
import { queuedSaveWarning } from "../store/clauseDelivery";
import { useCharter } from "../store/store";
import { AddSiteForm, DomainList, ScheduleEditor, Segmented, Toggle } from "./Limits";
import { AppPicker } from "./NamedTimes";
import { devicesLeftWithoutHours, windowsOnDevice } from "./timesView";
import { normalizeWebDomain } from "../domain/webDomain";

// The editor for ONE time (Learning time, or one the guardian made): what it is
// called, what it allows, whether it costs screen time, and — per device — the
// hours it runs. Opens as a page (`ward/<id>/time/<timeId>`, or `.../time/new/…`)
// with Cancel / Save pinned at the bottom, like every rule editor.
//
// A time's DEFINITION is per child (one `times` clause) and its HOURS are per
// device (windows tagged with its id, inside each device's own schedule). This
// page edits both and signs them as ONE change, so a device is never given hours
// for a time it has not been told about, nor a definition it cannot yet run.

/** A typed website as `DomainList` takes it: the bare host, or a sentence on why not. */
function parseSite(s: string): { ok: true; value: string } | { ok: false; message: string } {
  const r = normalizeWebDomain(s);
  return r.ok ? { ok: true, value: r.domain } : r;
}

const POSTURE_OPTIONS: { value: TimePosture; label: string }[] = [
  { value: "onlyThese", label: "Only these" },
  { value: "everythingExcept", label: "Everything except" },
];

const POSTURE_HELP: Record<TimePosture, string> = {
  onlyThese:
    "Only the apps and websites you choose can be used while this time runs. Everything else is closed.",
  everythingExcept:
    "Works like Free time: everything is open apart from what Free time bans. Add changes below to ban more, or to allow something Free time bans.",
};

/** A catalogue pick (or a site the guardian typed) as the wire's site app. The
 *  device validates the address as https and the domains as plain host names, so
 *  both are made so here rather than refused at the device. */
function toSite(a: { id: string; label: string; url?: string; domains?: string[] }): LearningApp {
  return {
    id: a.id,
    label: a.label,
    kind: "site",
    url: (a.url ?? "").replace(/^http:\/\//i, "https://"),
    domains: [...(a.domains ?? [])],
  };
}

/** Learning time starts from the sites the guardian already trusts for study
 *  (the Free-time "time-free" list), so the template is not blank. */
function seedSites(policy: Policy): LearningApp[] {
  return (policy.learning?.apps ?? [])
    .filter((a) => a.kind === "site" && a.url && (a.domains?.length ?? 0) > 0)
    .map(toSite)
    .filter((s) => siteFault(s) === null);
}

/** Does anything about this device's schedules differ between two policies?
 *  (Base schedule and each device's own.) */
function schedulesDiffer(a: Policy, b: Policy): boolean {
  const view = (p: Policy) =>
    JSON.stringify([
      p.schedule,
      Object.entries(p.deviceOverrides ?? {})
        .map(([id, o]) => [id, o.schedule])
        .sort(([x], [y]) => String(x).localeCompare(String(y))),
    ]);
  return view(a) !== view(b);
}

const LOCAL_TZ =
  (typeof Intl !== "undefined" && Intl.DateTimeFormat().resolvedOptions().timeZone) || "UTC";

/** The editor's working schedule for one time on one device: just that time's
 *  hours, as plain windows, so the existing schedule editor can drive it. */
function asSchedule(windows: TimeWindows, tz: string): Schedule {
  const weekly: Schedule["weekly"] = {};
  for (const d of WEEKDAYS) weekly[d] = (windows[d] ?? []).map((w) => ({ ...w }));
  return { tz, weekly, paused: false };
}

function fromSchedule(s: Schedule): TimeWindows {
  const out: TimeWindows = {};
  for (const d of WEEKDAYS as Weekday[]) {
    const ws = s.weekly[d] ?? [];
    if (ws.length > 0) out[d] = ws.map((w) => ({ start: w.start, end: w.end }));
  }
  return out;
}

export default function TimeEditor({
  childId,
  timeId,
  template,
  name,
  onDone,
}: {
  childId: string;
  /** An existing time's id; omitted when making a new one. */
  timeId?: string;
  template?: "learning" | "custom";
  /** The name typed for a custom time. */
  name?: string;
  onDone?: () => void;
}) {
  const { state } = useCharter();
  const child = state.children.find((c) => c.id === childId);
  if (!child) return null;
  const done = onDone ?? (() => (window.location.hash = `/${wardSettingsPath(child.id)}`));
  const policy = child.policies.find((p) => p.scope.kind === "device");
  if (!policy) {
    return (
      <Banner tone="warn">
        {child.name} has no charter to edit yet. Setting up a device creates one.
      </Banner>
    );
  }
  const existing = timeId ? policy.times?.find((t) => t.id === timeId) : undefined;
  if (timeId && !existing) {
    return (
      <Card>
        <p className="card-sub" style={{ marginBottom: 14 }}>
          That time isn’t set up any more.
        </p>
        <Button block onClick={done}>
          Back
        </Button>
      </Card>
    );
  }
  return (
    <TimeEditorForm
      child={child}
      policy={policy}
      existing={existing}
      template={template ?? "custom"}
      name={name}
      onDone={done}
    />
  );
}

function TimeEditorForm({
  child,
  policy,
  existing,
  template,
  name,
  onDone,
}: {
  child: Child;
  policy: Policy;
  existing?: NamedTime;
  template: "learning" | "custom";
  name?: string;
  onDone: () => void;
}) {
  const { savePolicy, deviceStatus, state } = useCharter();
  const isNew = !existing;
  const takenIds = useMemo(() => (policy.times ?? []).map((t) => t.id), [policy.times]);

  const initialTime = useMemo<NamedTime>(
    () =>
      existing ??
      (template === "learning"
        ? learningTimeTemplate(seedSites(policy))
        : customTimeTemplate(name ?? "", takenIds)),
    // The first render's definition is the baseline; later store updates (a
    // heartbeat) must not reset what the guardian is typing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );
  const [draft, setDraft] = useState<NamedTime>(initialTime);

  const paired = child.devices.filter((d) => d.pairing === "paired");
  const capable = paired.filter((d) => canUseTimes(d, deviceStatus));
  const tooOld = paired.filter((d) => !canUseTimes(d, deviceStatus));

  // Hours per device, as the guardian is editing them. Seeded from what the
  // device's schedule says now (nothing, for a new time).
  const initialWindows = useMemo(() => {
    const out: Record<string, TimeWindows> = {};
    for (const d of capable) out[d.id] = existing ? windowsOnDevice(policy, existing.id, d.id) : {};
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const [windows, setWindows] = useState<Record<string, TimeWindows>>(initialWindows);

  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [queuedNote, setQueuedNote] = useState<string | null>(null);
  const [siteError, setSiteError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const label = draft.label.trim();
  const finalTime = useMemo<NamedTime>(
    () => ({
      ...draft,
      label,
      // A new time's id is fixed at the moment it is saved, from the name as it
      // stands then; a rename of an existing time never touches it.
      id: isNew ? (template === "learning" ? "learning" : timeSlug(label, takenIds)) : draft.id,
    }),
    [draft, label, isNew, template, takenIds],
  );
  const problems = useMemo(() => namedTimeProblems(finalTime), [finalTime]);

  const changedDevices = capable.filter(
    (d) => JSON.stringify(windows[d.id] ?? {}) !== JSON.stringify(initialWindows[d.id] ?? {}),
  );
  const windowsChanged = changedDevices.length > 0;

  // The policy this save would produce, built with the domain's own functions so
  // every rule they enforce (a tagged window never reaches an old device, two
  // times cannot overlap on a device, a time must exist before it has hours) is
  // shown here as the sentence they throw, before the guardian presses Save.
  const built = useMemo<{ policy: Policy; error: null } | { policy: null; error: string }>(() => {
    try {
      let p: Policy = isNew ? addNamedTime(policy, finalTime) : updateNamedTime(policy, finalTime);
      for (const d of changedDevices) {
        p = setTimeWindows(p, finalTime.id, d.id, windows[d.id] ?? {}, child.devices, deviceStatus);
      }
      return { policy: p, error: null };
    } catch (e) {
      return { policy: null, error: e instanceof Error ? e.message : "Couldn’t build that time." };
    }
  }, [policy, finalTime, isNew, changedDevices, windows, child.devices, deviceStatus]);

  const dirty =
    isNew ||
    windowsChanged ||
    JSON.stringify(existing ? { ...existing, label: existing.label } : null) !==
      JSON.stringify({ ...draft, label: draft.label.trim() });
  const blocked = problems.length > 0 || built.error !== null;

  const sectionsAll = useMemo(() => appsBySection(child.devices, deviceStatus), [child.devices, deviceStatus]);
  const sections = sectionsAll.filter((s) => capable.some((d) => d.id === s.deviceId));
  const labelFor = (pkg: string) =>
    humanLabelFor(pkg) ??
    sectionsAll.flatMap((s) => s.apps).find((a) => a.pkg === pkg)?.label ??
    pkg;

  const patch = (p: Partial<NamedTime>) => setDraft((d) => ({ ...d, ...p }));
  const list = (key: "apps" | "blockApps" | "unblockApps" | "denySites" | "undenySites") => draft[key] ?? [];
  const addTo = (key: "apps" | "blockApps" | "unblockApps") => (pkg: string) =>
    patch({ [key]: list(key).includes(pkg) ? list(key) : [...list(key), pkg] });
  const removeFrom = (key: "apps" | "blockApps" | "unblockApps") => (pkg: string) =>
    patch({ [key]: list(key).filter((p) => p !== pkg) });

  const sites = draft.sites ?? [];
  const toggleCatalogue = (entry: (typeof LEARNING_CATALOGUE)[number], on: boolean) =>
    patch({ sites: on ? [...sites, toSite(entry)] : sites.filter((s) => s.id !== entry.id) });
  const customSites = sites.filter((s) => !LEARNING_CATALOGUE.some((c) => c.id === s.id));

  const { allowLeaving } = useLeaveGuard(
    dirty && !saving,
    `You have unsaved changes to ${label || "this time"}. Leave them unsaved?`,
  );
  const leave = () => {
    allowLeaving();
    onDone();
  };

  async function run(next: Parameters<typeof savePolicy>[2]) {
    setSaveError(null);
    setQueuedNote(null);
    setSaving(true);
    try {
      const delivery = await savePolicy(child.id, policy.id, next);
      if (delivery.outcome === "cancelled") return;
      const queued = queuedSaveWarning(delivery);
      setQueuedNote(queued);
      if (!queued && mounted.current) leave();
    } catch (e) {
      setSaveError(
        e instanceof Error && e.message
          ? `Couldn’t save these changes: ${e.message}`
          : "Couldn’t save these changes. Please try again.",
      );
    } finally {
      setSaving(false);
    }
  }

  async function save() {
    if (!built.policy || problems.length > 0) return;
    const p = built.policy;
    // Hours changed: sign the schedule dimension too (the per-device schedules
    // ride on it — a save that left `schedule` out would sign none of them).
    if (windowsChanged && !p.schedule) {
      setSaveError("Set up Free time’s schedule first, then give this time its hours.");
      return;
    }
    await run({
      times: p.times,
      ...(windowsChanged ? { schedule: p.schedule, deviceOverrides: p.deviceOverrides } : {}),
    });
  }

  // Deleting takes the time AND every window tagged with it off every device's
  // schedule in the same signed save, so no device is left locked on an id that
  // no longer exists.
  const afterDelete = useMemo(() => {
    if (!existing) return null;
    try {
      return removeNamedTime(policy, existing.id);
    } catch {
      return null;
    }
  }, [existing, policy]);
  const leftWithNothing = afterDelete ? devicesLeftWithoutHours(afterDelete, child.devices) : [];

  async function remove() {
    if (!afterDelete) return;
    const differ = schedulesDiffer(policy, afterDelete);
    if (differ && !afterDelete.schedule) {
      setSaveError("Couldn’t take this time’s hours off the schedule. Try again after setting up Free time.");
      return;
    }
    await run({
      times: afterDelete.times ?? [],
      ...(differ ? { schedule: afterDelete.schedule, deviceOverrides: afterDelete.deviceOverrides } : {}),
    });
  }

  const posture = draft.posture;
  const tzFor = (d: Device) => {
    const sched = policy.deviceOverrides?.[d.id]?.schedule ?? policy.schedule;
    return sched?.tz ?? LOCAL_TZ;
  };

  return (
    <div className="rule-page">
      <Card>
        <div className="field" style={{ margin: 0 }}>
          <label className="field-label" htmlFor="time-name">
            Name
          </label>
          <input
            id="time-name"
            className="input"
            value={draft.label}
            maxLength={60}
            placeholder="e.g. Learning time"
            onChange={(e) => patch({ label: e.target.value })}
          />
          {!isNew && (
            <p className="card-sub" style={{ margin: "6px 0 0" }}>
              Renaming only changes the name. Its hours and the time already given stay as they are.
            </p>
          )}
        </div>
      </Card>

      <h3 className="rule-group-title">What it allows</h3>
      <Card>
        <Segmented<TimePosture> value={posture} options={POSTURE_OPTIONS} onChange={(v) => patch({ posture: v })} />
        <p className="card-sub" style={{ margin: "8px 0 0" }}>
          {POSTURE_HELP[posture]}
        </p>

        {posture === "onlyThese" ? (
          <div className="stack" style={{ marginTop: 14 }}>
            <div>
              <div className="field-label">Apps</div>
              <AppPicker
                apps={list("apps")}
                sections={sections}
                labelFor={labelFor}
                onAdd={addTo("apps")}
                onRemove={removeFrom("apps")}
                wardName={child.name}
              />
            </div>
            <div>
              <div className="field-label">Websites</div>
              <div className="stack" style={{ gap: 6 }}>
                {LEARNING_CATALOGUE.map((c) => {
                  const on = sites.some((s) => s.id === c.id);
                  return (
                    <div key={c.id} className="row-between">
                      <span className="row-title">{c.label}</span>
                      <Toggle checked={on} label={`${c.label} in ${label || "this time"}`} onChange={(v) => toggleCatalogue(c, v)} />
                    </div>
                  );
                })}
                {customSites.map((s) => (
                  <div key={s.id} className="row-between">
                    <span className="row-main">
                      <span className="row-title">{s.label}</span>
                      <br />
                      <span className="row-sub">{(s.domains ?? []).join(", ")}</span>
                    </span>
                    <Button variant="ghost" onClick={() => patch({ sites: sites.filter((x) => x.id !== s.id) })}>
                      Remove
                    </Button>
                  </div>
                ))}
              </div>
              {siteError && (
                <p className="card-sub" style={{ color: "var(--danger)", margin: "6px 0 0" }} role="alert">
                  {siteError}
                </p>
              )}
              <AddSiteForm
                takenIds={sites.map((s) => s.id)}
                onAdd={(app) => {
                  const site = toSite(app);
                  const fault = siteFault(site);
                  if (fault) {
                    setSiteError(`That site can’t be added: ${fault}.`);
                    return;
                  }
                  setSiteError(null);
                  patch({ sites: [...sites, site] });
                }}
              />
            </div>
          </div>
        ) : (
          <div className="stack" style={{ marginTop: 14 }}>
            <div>
              <div className="field-label">Also block these apps</div>
              <AppPicker
                apps={list("blockApps")}
                sections={sections}
                labelFor={labelFor}
                onAdd={addTo("blockApps")}
                onRemove={removeFrom("blockApps")}
                wardName={child.name}
              />
            </div>
            <div>
              <div className="field-label">Allow these apps, even if Free time bans them</div>
              <AppPicker
                apps={list("unblockApps")}
                sections={sections}
                labelFor={labelFor}
                onAdd={addTo("unblockApps")}
                onRemove={removeFrom("unblockApps")}
                wardName={child.name}
              />
            </div>
            <DomainList
              label="Also block these websites"
              hint="Blocked while this time runs, on top of what Free time blocks."
              items={list("denySites")}
              placeholder="example.com"
              parse={parseSite}
              onChange={(next) => patch({ denySites: next })}
            />
            <DomainList
              label="Allow these websites, even if Free time blocks them"
              hint="Open while this time runs, though Free time blocks them."
              items={list("undenySites")}
              placeholder="example.com"
              parse={parseSite}
              onChange={(next) => patch({ undenySites: next })}
            />
          </div>
        )}
      </Card>

      <h3 className="rule-group-title">Screen time</h3>
      <Card>
        <div className="row-between">
          <div className="row-main">
            <span className="row-title">Uses up {child.name}’s screen time</span>
            <br />
            <span className="row-sub">
              {draft.countsAgainstBudget
                ? "Minutes here come out of the daily and weekly limit, and running out of it locks this time too."
                : "Minutes here don’t come out of the daily or weekly limit, and running out of it doesn’t lock this time."}
            </span>
          </div>
          <Toggle
            checked={draft.countsAgainstBudget === true}
            label="Uses up screen time"
            onChange={(on) => patch({ countsAgainstBudget: on })}
          />
        </div>
      </Card>

      <h3 className="rule-group-title">Hours</h3>
      {tooOld.length > 0 && (
        <Banner tone="warn">
          {tooOld.map((d) => d.label).join(" and ")} {tooOld.length > 1 ? "need" : "needs"} updating to use
          times, so {tooOld.length > 1 ? "they have" : "it has"} no hours here. Update{" "}
          {tooOld.length > 1 ? "them" : "it"} and this will turn on by itself.
        </Banner>
      )}
      {capable.length === 0 ? (
        <Card>
          <p className="card-sub" style={{ margin: 0 }}>
            No device can run this time yet. You can still save it; it starts working once a device is set
            up and up to date.
          </p>
        </Card>
      ) : (
        capable.map((d) => (
          <Card key={d.id}>
            <div className="row-sub" style={{ marginBottom: 6 }}>
              <span aria-hidden="true">{d.platform === "android" ? "📱" : "💻"}</span> {d.label}
            </div>
            <ScheduleEditor
              kind={label || "this time"}
              schedule={asSchedule(windows[d.id] ?? {}, tzFor(d))}
              onChange={(s) => setWindows((w) => ({ ...w, [d.id]: fromSchedule(s) }))}
            />
          </Card>
        ))
      )}
      {capable.length > 0 && (
        <p className="card-sub" style={{ margin: "0 4px 12px" }}>
          Where these hours overlap Free time’s, this time carves out of it. Two times can’t run at the same
          time on one device. With no hours at all, this time only runs when you give it More time.
        </p>
      )}

      {problems.length > 0 && (
        <div style={{ marginTop: 6 }}>
          <Banner tone="warn">
            {problems.map((p) => (
              <span key={p} style={{ display: "block" }}>
                {p}
              </span>
            ))}
          </Banner>
        </div>
      )}
      {built.error && (
        <div style={{ marginTop: 6 }} role="alert">
          <Banner tone="warn">{built.error}</Banner>
        </div>
      )}
      {saveError && (
        <div style={{ marginTop: 6 }}>
          <Banner tone="warn">{saveError}</Banner>
        </div>
      )}
      {queuedNote && !saveError && (
        <div style={{ marginTop: 6 }}>
          <Banner tone="warn">{queuedNote}</Banner>
        </div>
      )}

      {!isNew && (
        <div style={{ marginTop: 20 }}>
          {confirmDelete ? (
            <Card>
              <p className="card-sub" style={{ marginTop: 0 }}>
                Delete {existing?.label}? Its hours come off every device’s schedule in the same save.
              </p>
              {leftWithNothing.length > 0 && (
                <Banner tone="warn">
                  {leftWithNothing.map((d) => d.label).join(" and ")} would have no allowed time at all, so{" "}
                  {leftWithNothing.length > 1 ? "they’d" : "it’d"} stay locked all day until you add some.
                </Banner>
              )}
              <div className="stack" style={{ marginTop: 10 }}>
                <Button variant="danger" block disabled={saving} onClick={() => void remove()}>
                  {saving ? "Deleting…" : `Delete ${existing?.label}`}
                </Button>
                <Button variant="secondary" block disabled={saving} onClick={() => setConfirmDelete(false)}>
                  Keep it
                </Button>
              </div>
            </Card>
          ) : (
            <Button variant="ghost" block onClick={() => setConfirmDelete(true)}>
              Delete this time
            </Button>
          )}
        </div>
      )}

      <RuleSaveBar
        dirty={dirty}
        saving={saving}
        blocked={blocked}
        onSave={() => void save()}
        onCancel={leave}
        note={
          state.signer.connected && !state.signer.autoSign && dirty ? (
            <Banner tone="info">
              You’ll confirm these changes{state.signer.label ? ` with ${state.signer.label}` : ""} before they
              apply.
            </Banner>
          ) : undefined
        }
      />
    </div>
  );
}
