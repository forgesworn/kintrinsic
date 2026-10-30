// One bottom sheet for every lever over today: give time, stop now, and the
// amount on an ask for more time.
//
// These used to expand inline inside the ward's card, beside each other, so a
// stepper's "+" landed under the neighbouring button and "Give 30 min" wrapped
// to three lines. A full-width sheet with nothing laid out beside the stepper
// cannot overlap, whatever the screen.
//
// Three modes, each carrying over the store calls and error handling of the
// inline control it replaces:
//
//   give     replaces GiveTime. Three presets give straight away; the stepper
//            and the "Add it to" group picker are for anything else.
//   approve  replaces the stepper on a "more time" ask. Same store calls the
//            ask card makes: setRequestChosenMinutes, approveRequest, denyRequest.
//   stop     replaces FinishNow: warn-then-finish, or lift a stand-down.
//   retype   answers an ask with a DIFFERENT kind of time than it asked for
//            (Free time instead of Learning time, say): a 0-minute grant closes
//            the ask and a typed gift buys the minutes, both to the asking
//            device only.
//
// When the child has times (Learning time…), `give` also offers WHICH kind of
// time the minutes buy and WHICH devices get them: aimed by default at the
// devices that are locked, and at the kind the contract's ward-side rule picks.
// A child with no times sees exactly the sheet they always saw.
//
// `onClose` receives a one-line confirmation after a send, for the caller to
// show where the sheet was opened from.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Button } from "./ui";
import { useCharter } from "../store/store";
import { standingFor, standingNote } from "../domain/standing";
import { deliverableDevices, wardenSupport } from "../domain/wardenSupport";
import { canUseTimes, timesGate } from "../domain/times";
import { isAskExpired } from "../domain/askExpiry";
import type { Child, ChildRequest } from "../domain/types";
import { endAskDecision, tryBeginAskDecision } from "./askDecisionLock";
import {
  defaultGiftDeviceIds,
  defaultGiftTime,
  giftDevices,
  giftOptions,
  timeChoices,
} from "./timesLever";
import {
  DEFAULT_GIFT_MINUTES,
  GIFT_MAX,
  GIFT_MIN,
  GIFT_STEP,
  useGiveSupport,
  useStopState,
} from "./leverState";

export type LeverSheetProps =
  | { mode: "give"; child: Child; onClose: (note?: string) => void }
  | { mode: "stop"; child: Child; onClose: (note?: string) => void }
  | { mode: "retype"; child: Child; request: ChildRequest; onClose: (note?: string) => void }
  | { mode: "approve"; child: Child; request: ChildRequest; onClose: (note?: string) => void };

/** The give presets: each one gives that amount at once. */
const PRESETS: { minutes: number; label: string }[] = [
  { minutes: 15, label: "+15 min" },
  { minutes: 30, label: "+30 min" },
  { minutes: 60, label: "+1 hour" },
];

const WARN = { color: "var(--warn, #b45309)" } as const;

/** The backdrop and sheet, closing on backdrop tap and Escape (never while a
 *  send is in flight: a half-sent decision must not look abandoned). */
function SheetFrame({
  label,
  busy,
  onClose,
  children,
}: {
  label: string;
  busy: boolean;
  onClose: () => void;
  children: ReactNode;
}) {
  const sheetRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const busyRef = useRef(busy);
  busyRef.current = busy;

  useEffect(() => {
    sheetRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busyRef.current) closeRef.current();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div
      className="sheet-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={label}
      onClick={() => !busy && onClose()}
    >
      <div
        className="sheet lever-sheet"
        ref={sheetRef}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="card-title">{label}</h2>
        {children}
      </div>
    </div>
  );
}

/** `(−) 45 min (+)`, centred, with nothing beside it. */
function Stepper({
  minutes,
  min,
  max,
  disabled,
  onStep,
}: {
  minutes: number;
  min: number;
  max: number;
  disabled: boolean;
  onStep: (delta: number) => void;
}) {
  return (
    <div className="lever-stepper" role="group" aria-label="How many minutes">
      <Button
        variant="secondary"
        className="lever-step"
        aria-label="5 minutes less"
        disabled={disabled || minutes <= min}
        onClick={() => onStep(-GIFT_STEP)}
      >
        −
      </Button>
      <span className="lever-amount" aria-live="polite">
        {minutes} min
      </span>
      <Button
        variant="secondary"
        className="lever-step"
        aria-label="5 minutes more"
        disabled={disabled || minutes >= max}
        onClick={() => onStep(GIFT_STEP)}
      >
        +
      </Button>
    </div>
  );
}

function GiveBody({ child, onClose }: { child: Child; onClose: (note?: string) => void }) {
  const { giveTime, deviceStatus } = useCharter();
  const childName = child.name;
  const support = useGiveSupport(child);

  // What is actually true about this ward right now. Giving five minutes to a
  // ward an hour over, or with twenty minutes left in the week, is a different
  // decision, and the guardian should not have to go looking for it.
  const standing = useMemo(
    () => standingNote(standingFor(child, deviceStatus)),
    [child, deviceStatus],
  );

  // The saved policy's counted groups: the group picker's choices. A ward with
  // none (never used named times, every group free or on-request, or Counted
  // times PAUSED: a paused axis has nothing live to add minutes to) sees no
  // picker at all, and giving stays whole-day.
  const bucketsPolicy = child.policies.find((p) => p.scope.kind === "device")?.buckets;
  const countedGroups = bucketsPolicy?.enabled ? bucketsPolicy.buckets : [];
  // Whether SOME devices predate named times, shown only once a group is
  // picked. Checks both gates (buckets at all, and the weekly axis); either way
  // an older device falls back to the whole day.
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

  const [minutes, setMinutes] = useState(DEFAULT_GIFT_MINUTES);
  // undefined = "Whole day".
  const [groupId, setGroupId] = useState<string | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // ── Times: which kind of time, and which devices ─────────────────────────
  const devicePolicy = child.policies.find((p) => p.scope.kind === "device");
  const times = devicePolicy?.times;
  const choices = useMemo(() => timeChoices(times), [times]);
  const allPaired = useMemo(() => giftDevices(child.devices, deviceStatus, false), [child.devices, deviceStatus]);
  // Fixed when the sheet opens: the locked-ness and the schedule move on every
  // heartbeat, and a default that re-aimed itself under the guardian's thumb
  // would send the minutes somewhere they did not pick.
  const [opened] = useState(() => Date.now());
  const [timeId, setTimeId] = useState<string | undefined>(() =>
    defaultGiftTime(
      devicePolicy,
      defaultGiftDeviceIds(giftDevices(child.devices, deviceStatus, false), deviceStatus, opened)[0],
      opened,
    ),
  );
  // `null` = "the default for whatever kind is picked".
  const [chosen, setChosen] = useState<string[] | null>(null);
  const typed = timeId !== undefined;
  const candidates = useMemo(
    () => giftDevices(child.devices, deviceStatus, typed),
    [child.devices, deviceStatus, typed],
  );
  // Picking a kind resets `chosen`, so a typed gift never keeps a device that
  // cannot run it; unchecking every box leaves nobody, which blocks sending.
  const targetIds = useMemo(
    () =>
      chosen
        ? chosen.filter((id) => candidates.some((d) => d.id === id))
        : defaultGiftDeviceIds(candidates, deviceStatus, opened),
    [chosen, candidates, deviceStatus, opened],
  );
  const timeLabel = choices.find((c) => c.id === timeId)?.label;
  const showTimes = (times?.length ?? 0) > 0;
  const showDevices = allPaired.length > 1;
  const noTarget = targetIds.length === 0;

  const step = (delta: number) =>
    setMinutes((m) => Math.max(GIFT_MIN, Math.min(GIFT_MAX, m + delta)));

  const send = async (mins: number) => {
    if (busy || noTarget) return;
    setBusy(true);
    setError(null);
    try {
      const opts = giftOptions({
        timeId,
        timeLabel,
        chosenIds: targetIds,
        allIds: allPaired.map((d) => d.id),
      });
      // `opts` is passed only when the choice is not the old one, so a child
      // without times makes exactly the call this sheet always made.
      const ok = opts
        ? await giveTime(child.id, mins, typed ? undefined : groupId, opts)
        : await giveTime(child.id, mins, groupId);
      if (ok) {
        // Signed and on its way; if some devices are away from the home
        // network, say exactly who got it and who is still waiting.
        onClose(
          typeof ok === "object"
            ? `${mins} minutes${typed && timeLabel ? ` of ${timeLabel}` : ""} saved for ${childName}. ${ok.message}`
            : `${mins} minutes${typed && timeLabel ? ` of ${timeLabel}` : ""} sent to ${childName}.`,
        );
        return;
      }
      // Never claim minutes travelled when nothing was signed.
      setError("Turn on parent approval first, then try again.");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't send those minutes.");
    }
    setBusy(false);
  };

  return (
    <SheetFrame label={`Give ${childName} more time`} busy={busy} onClose={() => onClose()}>
      {!support.canSend ? (
        <>
          <p className="card-sub">
            {support.tooOld.map((d) => d.label).join(" and ")} needs the latest Kintrinsic
            before it can accept extra time. Update it, then this turns on by itself.
          </p>
          <Button variant="secondary" block onClick={() => onClose()}>
            Close
          </Button>
        </>
      ) : (
        <>
          {standing && (
            <p className="card-sub" style={{ margin: "0 0 10px", color: "var(--warn, #9a6516)" }}>
              {childName} is {standing}.
            </p>
          )}
          <div className="lever-presets" role="group" aria-label="Give straight away">
            {PRESETS.map((p) => (
              <Button
                key={p.minutes}
                variant="secondary"
                className="lever-preset"
                disabled={busy || noTarget}
                onClick={() => void send(p.minutes)}
              >
                {p.label}
              </Button>
            ))}
          </div>

          <p className="lever-or">Or choose:</p>
          <Stepper minutes={minutes} min={GIFT_MIN} max={GIFT_MAX} disabled={busy} onStep={step} />

          {/* Only when the child has times: which kind the minutes buy. Free time
              is the old behaviour. */}
          {showTimes && (
            <div style={{ marginTop: 14 }}>
              <label className="field-label" htmlFor={`give-time-${child.id}`} style={{ display: "block", marginBottom: 4 }}>
                Kind of time
              </label>
              <select
                id={`give-time-${child.id}`}
                value={timeId ?? ""}
                disabled={busy}
                onChange={(e) => {
                  setTimeId(e.target.value || undefined);
                  setChosen(null);
                  if (e.target.value) setGroupId(undefined);
                }}
                className="input"
              >
                {choices.map((c) => (
                  <option key={c.id ?? "free"} value={c.id ?? ""}>
                    {c.label}
                  </option>
                ))}
              </select>
            </div>
          )}

          {/* Which devices get it. Aimed at the locked ones by default; a typed
              gift lists only the devices that can run it. */}
          {showDevices && (
            <fieldset style={{ border: 0, padding: 0, margin: "14px 0 0" }}>
              <legend className="field-label" style={{ marginBottom: 4 }}>
                Give it to
              </legend>
              {giftDevices(child.devices, deviceStatus, false).map((d) => {
                const can = !typed || canUseTimes(d, deviceStatus);
                return (
                  <label
                    key={d.id}
                    style={{ display: "flex", alignItems: "center", gap: 10, minHeight: 44, opacity: can ? 1 : 0.55 }}
                  >
                    <input
                      type="checkbox"
                      checked={can && targetIds.includes(d.id)}
                      disabled={busy || !can}
                      onChange={(e) =>
                        setChosen(e.target.checked ? [...targetIds, d.id] : targetIds.filter((id) => id !== d.id))
                      }
                    />
                    <span>
                      <span aria-hidden="true">{d.platform === "android" ? "📱" : "💻"}</span> {d.label}
                      {!can && (
                        <span className="card-sub" style={{ display: "block", margin: 0 }}>
                          {timesGate(d, deviceStatus) === "unreported"
                            ? `Needs to check in once before it can run ${timeLabel ?? "this kind of time"}`
                            : `Needs updating to run ${timeLabel ?? "this kind of time"}`}
                        </span>
                      )}
                    </span>
                  </label>
                );
              })}
            </fieldset>
          )}
          {noTarget && (
            <p className="card-sub" style={{ ...WARN, marginTop: 8 }} role="alert">
              Choose a device to give it to.
            </p>
          )}

          {/* Optional, and only offered when the ward has a counted app group
              to pick (and the minutes are not for a kind of time — a gift tops
              up one pool, a group's or a time's): nothing to choose from is
              nothing to show. */}
          {countedGroups.length > 0 && !typed && (
            <div style={{ marginTop: 14 }}>
              <label
                className="field-label"
                htmlFor={`give-group-${child.id}`}
                style={{ display: "block", marginBottom: 4 }}
              >
                Add it to
              </label>
              <select
                id={`give-group-${child.id}`}
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
                  {oldGroupDevices.join(" and ")} needs the latest Kintrinsic to add this to{" "}
                  {countedGroups.find((g) => g.id === groupId)?.label} — older devices will add
                  it to the whole day instead.
                </p>
              )}
            </div>
          )}

          {error && (
            <p className="card-sub" style={{ ...WARN, marginTop: 10 }} role="alert">
              {error}
            </p>
          )}
          <Button
            block
            className="lever-primary"
            disabled={busy || noTarget}
            onClick={() => void send(minutes)}
            style={{ marginTop: 16 }}
          >
            {busy ? "Sending…" : `Give ${minutes} min${typed && timeLabel ? ` of ${timeLabel}` : ""}`}
          </Button>
          <p className="card-sub" style={{ marginTop: 10 }}>
            Extra time for today only — it doesn’t change their usual rules.
          </p>
        </>
      )}
    </SheetFrame>
  );
}

function ApproveBody({
  child,
  request,
  onClose,
}: {
  child: Child;
  request: ChildRequest;
  onClose: (note?: string) => void;
}) {
  const { approveRequest, denyRequest, setRequestChosenMinutes, deviceStatus } = useCharter();
  const [busy, setBusy] = useState<"approve" | "deny" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const asked = request.minutesRequested ?? 0;
  // Persisted on the REQUEST record, not sheet-local state: a local copy reset
  // to the full amount on remount, which is exactly what a failed send does. A
  // guardian who had dialled 30 down to 5 got a retry that silently granted 30.
  const granted = request.chosenMinutes ?? asked;
  // The ward's own broker ages an ask out after 24h and ignores a decision for
  // it, so neither answer is offered past that.
  const expired = isAskExpired(request.createdAt, Date.now());

  const standing = useMemo(
    () => standingNote(standingFor(child, deviceStatus)),
    [child, deviceStatus],
  );

  const step = (delta: number) =>
    setRequestChosenMinutes(request.id, Math.max(GIFT_MIN, Math.min(asked, granted + delta)));

  const decide = async (action: "approve" | "deny") => {
    // Shares the one-decision-at-a-time lock with the ask cards.
    if (busy) return;
    const token = tryBeginAskDecision();
    if (!token) return;
    setBusy(action);
    setError(null);
    try {
      if (action === "approve") await approveRequest(request.id, granted);
      else await denyRequest(request.id);
      onClose();
      return;
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't send your decision.");
    } finally {
      endAskDecision(token);
    }
    setBusy(null);
  };

  return (
    <SheetFrame label={`Give ${child.name} more time`} busy={busy !== null} onClose={() => onClose()}>
      {standing && (
        <p className="card-sub" style={{ margin: "0 0 10px", color: "var(--warn, #9a6516)" }}>
          {child.name} is {standing}.
        </p>
      )}
      {request.time && (
        <p className="card-sub" style={{ margin: "0 0 10px" }}>
          This is for{" "}
          {child.policies.find((p) => p.scope.kind === "device")?.times?.find((t) => t.id === request.time)
            ?.label ?? "a kind of time that isn’t set up any more"}
          .
        </p>
      )}
      <Stepper
        minutes={granted}
        min={GIFT_MIN}
        max={asked}
        disabled={busy !== null || expired}
        onStep={step}
      />
      {granted < asked && (
        <p className="card-sub" style={{ marginTop: 8, textAlign: "center" }}>
          They asked for {asked} min.
        </p>
      )}
      {expired && (
        <p className="card-sub" style={{ ...WARN, marginTop: 10 }}>
          This ask expired on the device — it’s more than a day old and the device won’t act on
          it now.
        </p>
      )}
      {error && (
        <p className="card-sub" style={{ ...WARN, marginTop: 10 }} role="alert">
          {error}
        </p>
      )}
      <div className="stack" style={{ marginTop: 16 }}>
        <Button
          block
          className="lever-primary"
          disabled={busy !== null || expired}
          onClick={() => void decide("approve")}
        >
          {busy === "approve" ? "Approving…" : expired ? "Expired" : `Approve ${granted} min`}
        </Button>
        <Button
          variant="secondary"
          block
          className="lever-primary"
          disabled={busy !== null || expired}
          onClick={() => void decide("deny")}
        >
          {busy === "deny" ? "Saving…" : "Not now"}
        </Button>
      </div>
    </SheetFrame>
  );
}

/**
 * Answer an ask with a different kind of time than it asked for. The ask was for
 * Learning time and the guardian would rather give Free time (or the other way
 * round): the ask is closed with a zero-minute grant and the minutes arrive as
 * a typed gift, both to the asking device alone — a sibling must not receive
 * time it never asked for.
 */
function RetypeBody({
  child,
  request,
  onClose,
}: {
  child: Child;
  request: ChildRequest;
  onClose: (note?: string) => void;
}) {
  const { answerRequestWithOtherTime } = useCharter();
  const times = child.policies.find((p) => p.scope.kind === "device")?.times;
  // Everything except what was asked for: approving as asked already covers it.
  const choices = useMemo(
    () => timeChoices(times).filter((c) => c.id !== request.time),
    [times, request.time],
  );
  const [timeId, setTimeId] = useState<string | undefined>(choices[0]?.id);
  const asked = request.minutesRequested ?? 0;
  const [minutes, setMinutes] = useState(request.chosenMinutes ?? asked);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const label = choices.find((c) => c.id === timeId)?.label ?? "Free time";
  const expired = isAskExpired(request.createdAt, Date.now());

  const send = async () => {
    if (busy) return;
    const token = tryBeginAskDecision();
    if (!token) return;
    setBusy(true);
    setError(null);
    try {
      await answerRequestWithOtherTime(request.id, minutes, timeId, label);
      onClose(`${minutes} minutes of ${label} sent instead.`);
      return;
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't send that.");
    } finally {
      endAskDecision(token);
    }
    setBusy(false);
  };

  return (
    <SheetFrame label="Give a different kind of time" busy={busy} onClose={() => onClose()}>
      <label className="field-label" htmlFor={`retype-${request.id}`} style={{ display: "block", marginBottom: 4 }}>
        Give {child.name} instead
      </label>
      <select
        id={`retype-${request.id}`}
        value={timeId ?? ""}
        disabled={busy || expired}
        onChange={(e) => setTimeId(e.target.value || undefined)}
        className="input"
      >
        {choices.map((c) => (
          <option key={c.id ?? "free"} value={c.id ?? ""}>
            {c.label}
          </option>
        ))}
      </select>
      <div style={{ marginTop: 14 }}>
        <Stepper
          minutes={minutes}
          min={GIFT_MIN}
          max={GIFT_MAX}
          disabled={busy || expired}
          onStep={(d) => setMinutes((m) => Math.max(GIFT_MIN, Math.min(GIFT_MAX, m + d)))}
        />
      </div>
      <p className="card-sub" style={{ marginTop: 8 }}>
        They asked for {asked} min
        {request.time ? " of another kind of time" : ""}. This answers the ask and sends the time to that
        device only.
      </p>
      {expired && (
        <p className="card-sub" style={{ ...WARN, marginTop: 10 }}>
          This ask expired on the device — it’s more than a day old and the device won’t act on it now.
        </p>
      )}
      {error && (
        <p className="card-sub" style={{ ...WARN, marginTop: 10 }} role="alert">
          {error}
        </p>
      )}
      <div className="stack" style={{ marginTop: 16 }}>
        <Button block className="lever-primary" disabled={busy || expired} onClick={() => void send()}>
          {busy ? "Sending…" : `Give ${minutes} min of ${label}`}
        </Button>
        <Button variant="secondary" block className="lever-primary" disabled={busy} onClick={() => onClose()}>
          Cancel
        </Button>
      </div>
    </SheetFrame>
  );
}

function StopBody({ child, onClose }: { child: Child; onClose: (note?: string) => void }) {
  const { standDown } = useCharter();
  const { canSend, tooOld, standing, quiet } = useStopState(child);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async (lift: boolean) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const ok = await standDown(child.id, lift);
      if (ok) {
        if (typeof ok === "object") {
          // Signed, but some devices are away from the home network: say who
          // got it and who is still waiting rather than the usual "sent" line.
          onClose(
            (lift
              ? `Saved — ${child.name}'s devices lift the lock when they hear this.`
              : `Saved — ${child.name} is asked to finish up.`) + ` ${ok.message}`,
          );
          return;
        }
        const waiting =
          quiet.length > 0
            ? ` ${quiet.map((d) => d.label).join(" and ")} ${
                quiet.length > 1 ? "are" : "is"
              } offline — ${quiet.length > 1 ? "they'll" : "it'll"} pick this up when back on.`
            : "";
        onClose(
          (lift
            ? `Sent — ${child.name}'s devices lift the lock when they hear this, usually under a minute.`
            : `${child.name} has a minute, then they're done for today.`) + waiting,
        );
        return;
      }
      // Never claim it travelled when nothing was signed.
      setError("Turn on parent approval first, then try again.");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't send that.");
    }
    setBusy(false);
  };

  // Refused only when NOTHING paired could honour it: otherwise the button
  // would be a lie. Anything else sends, and names any laggard below.
  if (!canSend) {
    return (
      <SheetFrame label={`Finish ${child.name} for today`} busy={false} onClose={() => onClose()}>
        <p className="card-sub">
          {tooOld.map((d) => d.label).join(" and ")} needs the latest Kintrinsic before it can be
          asked to finish up. Update it, then this turns on by itself.
        </p>
        <Button variant="secondary" block onClick={() => onClose()}>
          Close
        </Button>
      </SheetFrame>
    );
  }
  const laggards = tooOld.length > 0 ? tooOld.map((d) => d.label).join(" and ") : null;

  return (
    <SheetFrame
      label={standing ? `Allow ${child.name} back on` : `Finish ${child.name} for today`}
      busy={busy}
      onClose={() => onClose()}
    >
      {standing ? (
        <p className="card-sub" style={{ marginBottom: 12 }} aria-live="polite">
          Finished for today — {child.name} is waiting on you. Their devices lift the lock when
          they hear this, usually under a minute.
        </p>
      ) : (
        <>
          <p className="card-sub" style={{ marginBottom: 12 }}>
            {child.name} gets a minute’s warning, then their devices finish for the day. You can
            let them back on whenever you like.
          </p>
          {laggards && (
            <p className="card-sub" style={{ ...WARN, marginBottom: 12 }}>
              {laggards} is running an older Kintrinsic and will carry on until you update it —
              the rest will finish up.
            </p>
          )}
        </>
      )}
      {error && (
        <p className="card-sub" style={{ ...WARN, marginBottom: 12 }} role="alert">
          {error}
        </p>
      )}
      <div className="stack">
        <Button block className="lever-primary" disabled={busy} onClick={() => void run(standing)}>
          {busy ? "Sending…" : standing ? "Allow back on" : "Warn, then finish"}
        </Button>
        <Button
          variant="secondary"
          block
          className="lever-primary"
          disabled={busy}
          onClick={() => onClose()}
        >
          Cancel
        </Button>
      </div>
    </SheetFrame>
  );
}

export function LeverSheet(props: LeverSheetProps) {
  switch (props.mode) {
    case "give":
      return <GiveBody child={props.child} onClose={props.onClose} />;
    case "stop":
      return <StopBody child={props.child} onClose={props.onClose} />;
    case "approve":
      return <ApproveBody child={props.child} request={props.request} onClose={props.onClose} />;
    case "retype":
      return <RetypeBody child={props.child} request={props.request} onClose={props.onClose} />;
  }
}
