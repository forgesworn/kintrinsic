import { useEffect, useState } from "react";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";
import type { SendResult } from "../store/clauseDelivery";
import type { Child, Device } from "../domain/types";
import {
  DEBUGGING_HINT,
  DEBUGGING_WARNING,
  debuggingReading,
  debuggingSupport,
} from "../domain/debugging";
import { endOfTodayUnix, TETHER_WINDOWS } from "../domain/tetherWindow";
import { tooOldNote, wardenSupport } from "../domain/wardenSupport";

/**
 * One ward device's USB-debugging control: its state (Off / On until 18:00 / On
 * until you turn it off, from what the device reports; or "Turned on — waiting
 * for the device" where it has not caught up), a confirmed turn-on with a
 * window, and a one-tap turn off. Android wards that understand the clause only; a Linux laptop gets
 * nothing at all, and a phone reporting an older Kintrinsic gets one sentence.
 */
export default function DeviceDebugging({ child, device }: { child: Child; device: Device }) {
  const { deviceStatus, setDeviceDebugging } = useCharter();
  const [choosing, setChoosing] = useState(false);
  const [windowLabel, setWindowLabel] = useState(TETHER_WINDOWS[0].label);
  const [busy, setBusy] = useState(false);
  // The result of the send just made (the Developer options hint, or why it
  // failed). That the carrier is holding it for the home network is not here:
  // it is stored on the device record, so the headline keeps saying so after
  // you leave the screen.
  const [notice, setNotice] = useState<{ tone: "warn" | "ok"; text: string } | null>(null);

  // Re-render the moment a timed allowance runs out, so "On until 18:00" turns
  // to "Off" by itself instead of waiting for the next unrelated update.
  const [, setTick] = useState(0);
  const until = device.debugging?.allow ? device.debugging.until : undefined;
  useEffect(() => {
    if (until == null) return;
    const ms = until * 1000 - Date.now();
    if (ms <= 0) return;
    const id = setTimeout(() => setTick((n) => n + 1), Math.min(ms, 2 ** 31 - 1));
    return () => clearTimeout(id);
  }, [until]);

  const support = debuggingSupport(device, deviceStatus);
  if (support === "none") return null;
  if (support === "tooOld") {
    const note = tooOldNote(wardenSupport([device], deviceStatus, "debugging"), "turn USB debugging on");
    return note ? <p className="card-sub" style={{ flexBasis: "100%", margin: 0 }}>{note}</p> : null;
  }

  const nowUnix = Math.floor(Date.now() / 1000);
  const status = device.devicePubkey ? deviceStatus[device.devicePubkey] : undefined;
  const reading = debuggingReading(device.debugging, status, nowUnix);
  const on = reading.offerOff;

  const send = async (grant: { allow: boolean; until?: number }) => {
    setBusy(true);
    setNotice(null);
    try {
      const sent: SendResult = await setDeviceDebugging(child.id, device.id, grant);
      if (sent === true) {
        setNotice(grant.allow ? { tone: "ok", text: DEBUGGING_HINT } : null);
      } else if (sent) {
        setNotice({ tone: "warn", text: sent.message });
      }
    } catch (err) {
      setNotice({
        tone: "warn",
        text: err instanceof Error ? err.message : "Couldn’t change USB debugging. Try again.",
      });
    } finally {
      setBusy(false);
    }
  };

  const turnOn = async () => {
    const w = TETHER_WINDOWS.find((x) => x.label === windowLabel) ?? TETHER_WINDOWS[0];
    const until =
      w.minutes === -1
        ? undefined
        : w.minutes == null
          ? endOfTodayUnix()
          : Math.floor(Date.now() / 1000) + w.minutes * 60;
    setChoosing(false);
    await send(until == null ? { allow: true } : { allow: true, until });
  };

  return (
    <div style={{ flexBasis: "100%" }}>
      <div className="row-between" style={{ gap: 8 }}>
        <div>
          <div className="row-sub">USB debugging</div>
          <div className="card-sub" aria-live="polite">
            {reading.label}
          </div>
          {reading.note && <div className="card-sub">{reading.note}</div>}
        </div>
        {on ? (
          <Button
            variant="ghost"
            disabled={busy}
            aria-label={`Turn off USB debugging on ${device.label}`}
            onClick={() => void send({ allow: false })}
          >
            Turn off
          </Button>
        ) : (
          !choosing && (
            <Button
              variant="ghost"
              disabled={busy}
              aria-label={`Turn on USB debugging on ${device.label}`}
              onClick={() => {
                setNotice(null);
                setChoosing(true);
              }}
            >
              Turn on
            </Button>
          )
        )}
      </div>
      {choosing && !on && (
        <div className="stack" style={{ marginTop: 8, gap: 10 }}>
          <div role="group" aria-label={`How long to leave USB debugging on for ${device.label}`} style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
            {TETHER_WINDOWS.map((w) => {
              const picked = w.label === windowLabel;
              return (
                <button
                  key={w.label}
                  type="button"
                  aria-pressed={picked}
                  onClick={() => setWindowLabel(w.label)}
                  style={{
                    padding: "8px 12px",
                    borderRadius: "var(--radius-control)",
                    border: `1px solid ${picked ? "var(--brand)" : "var(--hairline, #2a3350)"}`,
                    background: picked ? "var(--brand-bg)" : "transparent",
                    color: picked ? "var(--brand)" : "var(--text)",
                    fontWeight: picked ? 700 : 500,
                    cursor: "pointer",
                  }}
                >
                  {w.label}
                </button>
              );
            })}
          </div>
          <Banner tone="warn">{DEBUGGING_WARNING}</Banner>
          <div className="row-between" style={{ gap: 8 }}>
            <Button variant="secondary" block onClick={() => setChoosing(false)}>
              Keep it off
            </Button>
            <Button variant="danger" block disabled={busy} onClick={() => void turnOn()}>
              Turn on USB debugging
            </Button>
          </div>
        </div>
      )}
      {notice && (
        <p
          className="card-sub"
          role={notice.tone === "warn" ? "alert" : "status"}
          style={{ margin: "4px 0 0", color: notice.tone === "warn" ? "var(--warn, #8a5a00)" : undefined }}
        >
          {notice.text}
        </p>
      )}
    </div>
  );
}
