import { useEffect, useState } from "react";
import type React from "react";
import { Button } from "../components/ui";
import { useCharter } from "../store/store";
import type { Child, Device } from "../domain/types";
import {
  isValidScreenLockPin,
  NO_SCREEN_LOCK_WARNING,
  SCREEN_LOCK_WINDOW_SECS,
  screenLockReading,
  screenLockSupport,
} from "../domain/screenLock";

type Choice = "set" | "remove";

/** Mask the PIN as it is typed without making it a password field: browsers
 *  and password managers offer to save anything typed into `type="password"`,
 *  and this PIN belongs to the ward's phone, not to the guardian's accounts. */
const PIN_MASK = { WebkitTextSecurity: "disc" } as React.CSSProperties;

/**
 * One ward phone's screen-lock card: whether it has a lock, a "Reset screen
 * lock" button that sets a new PIN or removes the lock (always shown to the ward and recorded on their phone),
 * and what became of the reset just sent, from the ward's own report. Android
 * wards that report `screenLockSecure` only; everything else shows nothing. A
 * phone Android won't let Kintrinsic reset (`screenLockResetSupported: false`)
 * gets no button, and says why.
 *
 * The PIN is held in this dialog's own state while it is typed and cleared the
 * moment it is sent or the dialog closes. It never reaches the store, which is
 * handed it for the one call and keeps only the reset's id and time. Its fields
 * are plain masked text with autofill off, so no browser or password manager
 * offers to save it.
 */
export default function DeviceScreenLock({ child, device }: { child: Child; device: Device }) {
  const { deviceStatus, resetDeviceScreenLock } = useCharter();
  const [open, setOpen] = useState(false);
  const [choice, setChoice] = useState<Choice>("set");
  const [screenLockPin, setScreenLockPin] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-render when the hour a reset can still be accepted runs out, so "Waiting
  // for the device…" (or an unconfirmed "received") turns into the plain
  // "didn't respond" (or "didn't confirm") by itself.
  const [, setTick] = useState(0);
  const sentAt = device.screenLockReset?.issuedAt;
  useEffect(() => {
    if (sentAt == null) return;
    const ms = (sentAt + SCREEN_LOCK_WINDOW_SECS) * 1000 - Date.now();
    if (ms <= 0) return;
    const id = setTimeout(() => setTick((n) => n + 1), Math.min(ms, 2 ** 31 - 1));
    return () => clearTimeout(id);
  }, [sentAt]);

  if (screenLockSupport(device, deviceStatus) === "hidden") return null;

  const status = device.devicePubkey ? deviceStatus[device.devicePubkey] : undefined;
  const reading = screenLockReading(
    device.screenLockReset,
    status,
    Math.floor(Date.now() / 1000),
    child.name,
  );

  const close = () => {
    setOpen(false);
    setScreenLockPin("");
    setAgain("");
    setChoice("set");
  };

  const mismatch = again !== "" && again !== screenLockPin;
  const canSend =
    !busy && (choice === "remove" || (isValidScreenLockPin(screenLockPin) && screenLockPin === again));

  const send = async () => {
    if (!canSend) return;
    const newPin = choice === "remove" ? "" : screenLockPin;
    setBusy(true);
    setError(null);
    // Clear what was typed before the send starts: the value is already in hand.
    close();
    try {
      await resetDeviceScreenLock(child.id, device.id, newPin);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Couldn’t send that. Try again.");
    } finally {
      setBusy(false);
    }
  };

  const progress = reading.progress;

  return (
    <div style={{ flexBasis: "100%" }}>
      <div className="row-between" style={{ gap: 8 }}>
        <div>
          <div className="row-sub">Screen lock</div>
          <div className="card-sub">{reading.state}</div>
          {reading.noLock && <div className="card-sub">{NO_SCREEN_LOCK_WARNING}</div>}
          {reading.notReadyNote && <div className="card-sub">{reading.notReadyNote}</div>}
          {reading.unsupportedNote && <div className="card-sub">{reading.unsupportedNote}</div>}
        </div>
        {reading.supported && (
          <Button
            variant="ghost"
            disabled={busy || !reading.ready}
            aria-label={`Reset screen lock on ${device.label}`}
            onClick={() => {
              setError(null);
              setOpen(true);
            }}
          >
            Reset screen lock
          </Button>
        )}
      </div>
      {progress && (
        <p
          className="card-sub"
          role={progress.tone === "warn" ? "alert" : "status"}
          style={{
            margin: "4px 0 0",
            color: progress.tone === "warn" ? "var(--warn, #8a5a00)" : undefined,
          }}
        >
          {progress.text}
        </p>
      )}
      {error && (
        <p className="card-sub" role="alert" style={{ margin: "4px 0 0", color: "var(--warn, #8a5a00)" }}>
          {error}
        </p>
      )}

      {open && reading.supported && (
        <div
          className="sheet-backdrop"
          role="dialog"
          aria-modal="true"
          aria-label={`Reset screen lock on ${device.label}`}
          onClick={close}
        >
          <div className="sheet" onClick={(e) => e.stopPropagation()}>
            <h2 className="card-title">Reset screen lock</h2>
            <div
              role="radiogroup"
              aria-label="What to do with the screen lock"
              className="stack"
              style={{ gap: 8, marginBottom: 12 }}
            >
              <label style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <input
                  type="radio"
                  name="screen-lock-choice"
                  checked={choice === "set"}
                  onChange={() => setChoice("set")}
                />
                Set a new PIN
              </label>
              <label style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <input
                  type="radio"
                  name="screen-lock-choice"
                  checked={choice === "remove"}
                  onChange={() => setChoice("remove")}
                />
                Remove the screen lock
              </label>
            </div>
            {choice === "set" && (
              <div className="stack" style={{ gap: 8, marginBottom: 12 }}>
                <label className="stack" style={{ gap: 4 }}>
                  <span className="row-sub">New PIN (4 to 16 digits)</span>
                  <input
                    type="text"
                    inputMode="numeric"
                    autoComplete="off"
                    spellCheck={false}
                    data-lpignore="true"
                    data-1p-ignore="true"
                    data-bwignore="true"
                    style={PIN_MASK}
                    maxLength={16}
                    aria-label="New PIN"
                    value={screenLockPin}
                    onChange={(e) => setScreenLockPin(e.target.value.replace(/\D/g, "").slice(0, 16))}
                  />
                </label>
                <label className="stack" style={{ gap: 4 }}>
                  <span className="row-sub">Enter it again</span>
                  <input
                    type="text"
                    inputMode="numeric"
                    autoComplete="off"
                    spellCheck={false}
                    data-lpignore="true"
                    data-1p-ignore="true"
                    data-bwignore="true"
                    style={PIN_MASK}
                    maxLength={16}
                    aria-label="New PIN again"
                    value={again}
                    onChange={(e) => setAgain(e.target.value.replace(/\D/g, "").slice(0, 16))}
                  />
                </label>
                {mismatch && (
                  <p className="card-sub" role="alert" style={{ margin: 0 }}>
                    The two PINs don’t match.
                  </p>
                )}
              </div>
            )}
            <p className="card-sub">{child.name}’s phone will tell them, and keep a record of it.</p>
            <div className="stack" style={{ marginTop: 6 }}>
              <Button variant="danger" block disabled={!canSend} onClick={() => void send()}>
                {choice === "remove" ? "Remove the screen lock" : "Set the new PIN"}
              </Button>
              <Button variant="secondary" block onClick={close}>
                Cancel
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
