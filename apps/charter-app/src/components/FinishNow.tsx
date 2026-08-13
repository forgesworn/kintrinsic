// Ask a ward to finish up now — the inverse of Give time.
//
// decented, 2026-07-27: "I'd like to be able to give them a 1 min warning, then
// lock them out for the rest of the day, or until allowed back on."
//
// One tap warns them and then locks. The ward keeps a real minute (the device
// starts that clock when it first sees the clause, so relay lag can't eat it),
// watches it count down, and lands on the SAME lock a spent budget gives —
// learning apps, the lifeline and break-glass all unchanged. There is
// deliberately no second kind of lock: a new lock path is where trapping a
// child becomes possible.
//
// It stays locked until the guardian lifts it, and lapses by itself at the end
// of the ward's day so a forgotten stand-down never runs into tomorrow. Give
// time cannot undo it — that ordering is enforced in the device's enforcer,
// not here. Every paired device is in scope (phones AND charterd laptops):
// the gate logic lives in domain/standdownGate.ts.

import { useMemo, useState } from "react";
import { Button } from "./ui";
import { useCharter } from "../store/store";
import { STATUS_FRESHNESS_SECS } from "../store/liveStatus";
import type { Child } from "../domain/types";
import { standingStandDown } from "../domain/standdownGate";
import { deliverableDevices, wardenSupport } from "../domain/wardenSupport";

export function FinishNow({ child }: { child: Child }) {
  const { standDown, deviceStatus } = useCharter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);

  const devices = useMemo(() => deliverableDevices(child.devices), [child.devices]);
  // A device that is simply switched off must never block this: the clause
  // waits on the relay and lands when it comes back. Only a device that
  // REPORTS an old Kintrinsic is known to be unable — see domain/wardenSupport.
  const support = useMemo(
    () => wardenSupport(devices, deviceStatus, "standdown"),
    [devices, deviceStatus],
  );

  // Whether one STANDS is the DEVICE's answer, never a guess from local state
  // — and only a FRESH answer: a device that went dark mid-stand-down must not
  // pin "waiting on you" past the midnight lapse.
  const standing = standingStandDown(devices, deviceStatus, Date.now());

  /** Devices we have not heard from lately — they get it when they're back. */
  const quiet = devices.filter((d) => {
    const s = deviceStatus[d.devicePubkey as string];
    return !s || Date.now() / 1000 - s.ts > STATUS_FRESHNESS_SECS;
  });

  const run = async (lift: boolean) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const ok = await standDown(child.id, lift);
      if (ok) {
        setConfirming(false);
        const waiting =
          quiet.length > 0
            ? ` ${quiet.map((d) => d.label).join(" and ")} ${
                quiet.length > 1 ? "are" : "is"
              } offline — ${quiet.length > 1 ? "they'll" : "it'll"} pick this up when back on.`
            : "";
        setNote(
          (lift
            ? `Sent — ${child.name}'s devices lift the lock when they hear this, usually under a minute.`
            : `${child.name} has a minute, then they're done for today.`) + waiting,
        );
      } else {
        // Never claim it travelled when nothing was signed.
        setError("Turn on parent approval first, then try again.");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : "Couldn't send that.");
    } finally {
      setBusy(false);
    }
  };

  // Refused only when NOTHING paired could honour it — otherwise the button
  // would be a lie. Anything else sends, and names any laggard below.
  if (!support.canSend) {
    return (
      <div style={{ marginTop: 10 }}>
        <Button variant="secondary" disabled>
          Finish now
        </Button>
        <p className="card-sub" style={{ marginTop: 6 }}>
          {support.tooOld.map((d) => d.label).join(" and ")} needs the latest
          Kintrinsic before it can be asked to finish up. Update it, then this
          turns on by itself.
        </p>
      </div>
    );
  }
  const laggards =
    support.tooOld.length > 0 ? support.tooOld.map((d) => d.label).join(" and ") : null;

  if (standing) {
    return (
      <div style={{ marginTop: 10 }}>
        <p className="card-sub" style={{ marginBottom: 6 }} aria-live="polite">
          Finished for today — {child.name} is waiting on you.
        </p>
        <Button variant="secondary" disabled={busy} onClick={() => run(true)}>
          {busy ? "Sending…" : "Allow back on"}
        </Button>
        {note && (
          <p className="card-sub" style={{ marginTop: 6 }} aria-live="polite">
            {note}
          </p>
        )}
        {error && (
          <p className="card-sub" style={{ color: "var(--warn, #b45309)" }} role="alert">
            {error}
          </p>
        )}
      </div>
    );
  }

  return (
    <div style={{ marginTop: 10 }}>
      {confirming ? (
        <>
          <p className="card-sub" style={{ marginBottom: 8 }}>
            {child.name} gets a minute’s warning, then their devices finish for
            the day. You can let them back on whenever you like.
          </p>
          {laggards && (
            <p className="card-sub" style={{ marginBottom: 8, color: "var(--warn, #b45309)" }}>
              {laggards} is running an older Kintrinsic and will carry on until you
              update it — the rest will finish up.
            </p>
          )}
          <div style={{ display: "flex", gap: 10 }}>
            <Button disabled={busy} onClick={() => run(false)}>
              {busy ? "Sending…" : "Warn, then finish"}
            </Button>
            <Button variant="secondary" disabled={busy} onClick={() => setConfirming(false)}>
              Cancel
            </Button>
          </div>
        </>
      ) : (
        <Button
          variant="secondary"
          onClick={() => {
            setConfirming(true);
            setNote(null);
          }}
        >
          Finish now
        </Button>
      )}
      {note && !confirming && (
        <p className="card-sub" style={{ marginTop: 6 }} aria-live="polite">
          {note}
        </p>
      )}
      {error && (
        <p className="card-sub" style={{ color: "var(--warn, #b45309)" }} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
