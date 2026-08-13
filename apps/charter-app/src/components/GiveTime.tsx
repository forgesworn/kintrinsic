// Give a child time without them asking for it.
//
// Answering an ask used to be the only way to add minutes — a GRANT must echo
// a pending request — so a parent who simply wanted to say "finish the level"
// had one lever: edit the schedule. That rewrites a standing rule to solve a
// one-off, and it does nothing for the case decented actually hit (2026-07-26):
// having said no, and then wanting to change their mind.
//
// Same shape as the approve stepper on Approvals, deliberately: opens at 30
// minutes, ±5, so it's one control to learn rather than two.

import { useMemo, useState } from "react";
import { Button } from "./ui";
import { useCharter } from "../store/store";
import { deliverableDevices, wardenSupport } from "../domain/wardenSupport";
import { standingFor, standingNote } from "../domain/standing";
import type { Child } from "../domain/types";

/** Where the stepper opens — the same opening ask a locked ward sends. */
export const DEFAULT_GIFT_MINUTES = 30;
const STEP = 5;
const MIN = 5;
const MAX = 240;

export function GiveTime({ child }: { child: Child }) {
  const { giveTime, deviceStatus } = useCharter();
  const childId = child.id;
  const childName = child.name;

  // Offered unless NOTHING paired could apply it. A device that simply hasn't
  // reported — switched off, out of signal — is assumed capable: the clause
  // waits on the relay for it. (This used to grey the button out for a quiet
  // device, which refused the action for the very reason it exists; see
  // domain/wardenSupport.) Every platform is counted, on its own version
  // scheme, so a laptop is no longer silently exempt from the check.
  // What is actually true about this ward right now. Giving five minutes to a
  // ward an hour over — or with twenty minutes left in the whole week — is a
  // different decision, and the guardian should not have to go looking for it.
  const note2 = useMemo(
    () => standingNote(standingFor(child, deviceStatus)),
    [child, deviceStatus],
  );

  const support = useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "gift"),
    [child.devices, deviceStatus],
  );
  const ready = support.canSend;

  // The saved policy's counted groups — the group picker's choices. A ward
  // with none (never used named times, every group is free/on-request, or
  // Counted times is currently PAUSED — `buckets.enabled === false`, review
  // round 1 fix: a paused axis has nothing live to add minutes to, so
  // offering it here would promise something the device isn't enforcing at
  // all) sees no picker at all: giving stays exactly today's whole-day
  // behaviour.
  const bucketsPolicy = child.policies.find((p) => p.scope.kind === "device")?.buckets;
  const countedGroups = bucketsPolicy?.enabled ? bucketsPolicy.buckets : [];
  // Whether SOME of this ward's devices predate named times — shown only
  // once a group is actually picked, since it's only then that it matters.
  // Checks BOTH gates: a device too old for buckets at all, or old enough
  // for buckets but not the weekly axis, either way falls back the same way
  // (errs generous — the whole family's default, not per-axis).
  const bucketSupport = useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "buckets"),
    [child.devices, deviceStatus],
  );
  const bucketsWeeklySupport = useMemo(
    () => wardenSupport(deliverableDevices(child.devices), deviceStatus, "bucketsWeekly"),
    [child.devices, deviceStatus],
  );
  const oldGroupDevices = [
    ...new Set([...bucketSupport.tooOld, ...bucketsWeeklySupport.tooOld].map((d) => d.label)),
  ];

  const [open, setOpen] = useState(false);
  const [minutes, setMinutes] = useState(DEFAULT_GIFT_MINUTES);
  // undefined = "Whole day" — exactly today's behaviour.
  const [groupId, setGroupId] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const step = (delta: number) =>
    setMinutes((m) => Math.max(MIN, Math.min(MAX, m + delta)));

  const send = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const ok = await giveTime(childId, minutes, groupId);
      if (ok) {
        setNote(`${minutes} minutes sent to ${childName}.`);
        setOpen(false);
        setMinutes(DEFAULT_GIFT_MINUTES);
        setGroupId(undefined);
      } else {
        // Never claim minutes travelled when nothing was signed.
        setError("Turn on parent approval first, then try again.");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't send those minutes.");
    } finally {
      setBusy(false);
    }
  };

  if (!ready) {
    return (
      <div style={{ marginTop: 10 }}>
        <Button variant="secondary" disabled>
          Give time
        </Button>
        <p className="card-sub" style={{ marginTop: 6 }}>
          {support.tooOld.map((d) => d.label).join(" and ")} needs the latest
          Kintrinsic before it can accept extra time. Update it, then this turns on
          by itself.
        </p>
      </div>
    );
  }

  if (!open) {
    return (
      <div style={{ marginTop: 10 }}>
        <Button variant="secondary" onClick={() => { setOpen(true); setNote(null); }}>
          Give time
        </Button>
        {note && (
          <p className="card-sub" style={{ marginTop: 6 }} aria-live="polite">
            {note}
          </p>
        )}
      </div>
    );
  }

  return (
    <div style={{ marginTop: 10 }}>
      <div className="field-label" id={`give-label-${childId}`}>
        Give {childName}
      </div>
      {note2 && (
        <p
          className="card-sub"
          style={{ margin: "0 0 8px", color: "var(--warn, #9a6516)" }}
        >
          {childName} is {note2}.
        </p>
      )}
      <div
        className="row-between"
        role="group"
        aria-labelledby={`give-label-${childId}`}
        style={{ justifyContent: "flex-start", gap: 14 }}
      >
        <Button
          variant="secondary"
          aria-label="5 minutes less"
          disabled={busy || minutes <= MIN}
          onClick={() => step(-STEP)}
          style={{ minWidth: 52, padding: 0 }}
        >
          −
        </Button>
        <span
          aria-live="polite"
          style={{ minWidth: 92, textAlign: "center", fontWeight: 700, fontSize: 18 }}
        >
          {minutes} min
        </span>
        <Button
          variant="secondary"
          aria-label="5 minutes more"
          disabled={busy || minutes >= MAX}
          onClick={() => step(STEP)}
          style={{ minWidth: 52, padding: 0 }}
        >
          +
        </Button>
      </div>
      <p className="card-sub" style={{ marginTop: 8 }}>
        Extra time for today only — it doesn’t change their usual rules.
      </p>
      {/* Optional — default is the whole day, exactly the behaviour above.
          Only offered when the ward actually has a counted named time to
          pick: nothing to choose from is nothing to show. */}
      {countedGroups.length > 0 && (
        <div style={{ marginTop: 10 }}>
          <label
            className="field-label"
            htmlFor={`give-group-${childId}`}
            style={{ display: "block", marginBottom: 4 }}
          >
            Add it to
          </label>
          <select
            id={`give-group-${childId}`}
            value={groupId ?? ""}
            disabled={busy}
            onChange={(e) => setGroupId(e.target.value || undefined)}
            className="input"
          >
            <option value="">Whole day</option>
            {countedGroups.map((g) => (
              <option key={g.id} value={g.id}>
                {g.label}
              </option>
            ))}
          </select>
          {groupId && oldGroupDevices.length > 0 && (
            <p className="card-sub" style={{ marginTop: 6 }}>
              {oldGroupDevices.join(" and ")} needs the latest Kintrinsic to add
              this to {countedGroups.find((g) => g.id === groupId)?.label} —
              older devices will add it to the whole day instead.
            </p>
          )}
        </div>
      )}
      {error && (
        <p className="card-sub" style={{ color: "var(--warn, #b45309)" }} role="alert">
          {error}
        </p>
      )}
      <div style={{ display: "flex", gap: 10, marginTop: 8 }}>
        <Button onClick={send} disabled={busy}>
          {busy ? "Sending…" : `Give ${minutes} min`}
        </Button>
        <Button variant="secondary" onClick={() => setOpen(false)} disabled={busy}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
