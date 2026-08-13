import { useState } from "react";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";
import { loadOrCreateGuardianKey } from "../signer/guardianKey";
import type { Child, Device } from "../domain/types";
import {
  computeUnlockView,
  formatUnlockCode,
  UNLOCK_CHALLENGE_LEN,
} from "./offlineUnlock";

/**
 * Offline unlock — a bottom sheet (matches SignSheet) that lets the guardian
 * open a locked device with NO network. The device's lock screen shows a short
 * 4-character challenge; the guardian types it here and reads back an 8-digit
 * code to type into the device. The code is derived from the guardian key
 * (`unlockCodeForDevice`), so the ward can't produce it themselves.
 *
 * The key is read locally only when `signer.kind === "local"` — the fully-gated
 * path inside `computeUnlockView`. On a Signet/bunker signer the raw key isn't
 * here, so we show a graceful "not on this signer yet" note instead of a code.
 * The code is ephemeral UI state: never persisted, never logged.
 */
export default function UnlockDevice({
  child,
  device,
  onClose,
}: {
  child: Child;
  device: Device;
  onClose: () => void;
}) {
  const { state } = useCharter();
  const [challengeInput, setChallengeInput] = useState("");

  const view = computeUnlockView({
    signerKind: state.signer.kind,
    devicePubkey: device.devicePubkey,
    rawChallenge: challengeInput,
    loadGuardianSk: loadOrCreateGuardianKey,
  });

  return (
    <div
      className="sheet-backdrop"
      role="dialog"
      aria-modal="true"
      aria-label={`Unlock ${device.label}`}
      onClick={onClose}
    >
      <div className="sheet" onClick={(e) => e.stopPropagation()}>
        <h2 className="card-title" style={{ marginTop: 0 }}>
          Unlock {device.label}
        </h2>

        {view.kind === "not-local" ? (
          <>
            <Banner tone="info">
              Offline unlock uses your guardian key, which lives with the signer
              you paired — it isn't available on this signer yet. Unlock from the
              device that holds your key, or turn on parent approval on this
              phone.
            </Banner>
            <div className="stack" style={{ marginTop: 16 }}>
              <Button block onClick={onClose}>
                Close
              </Button>
            </div>
          </>
        ) : view.kind === "no-pubkey" ? (
          <>
            <Banner tone="warn">
              This device hasn't finished pairing, so there's no key to build an
              unlock code from. Reconnect it, then try again.
            </Banner>
            <div className="stack" style={{ marginTop: 16 }}>
              <Button block onClick={onClose}>
                Close
              </Button>
            </div>
          </>
        ) : (
          <>
            <p className="card-sub" style={{ marginTop: 0, marginBottom: 16 }}>
              No internet needed. Read the short code on {child.name}'s locked
              screen, type it in below, then read back the 8-digit answer and
              type it into the device to open it.
            </p>

            <label className="field" htmlFor="unlock-challenge" style={{ margin: 0 }}>
              <span className="field-label">The code on the device's screen</span>
              <input
                id="unlock-challenge"
                className="input"
                autoComplete="off"
                autoCapitalize="characters"
                autoCorrect="off"
                spellCheck={false}
                inputMode="text"
                maxLength={UNLOCK_CHALLENGE_LEN}
                placeholder="e.g. K7QN"
                aria-describedby="unlock-answer"
                value={view.challenge}
                onChange={(e) => setChallengeInput(e.target.value)}
                style={{
                  fontFamily: "ui-monospace, monospace",
                  fontSize: 22,
                  letterSpacing: "0.25em",
                  textTransform: "uppercase",
                }}
              />
            </label>

            <div id="unlock-answer" aria-live="polite" style={{ marginTop: 18 }}>
              {view.kind === "code" ? (
                <div
                  style={{
                    padding: "18px 14px",
                    borderRadius: "var(--radius-card)",
                    background: "var(--surface-2)",
                    border: "1px solid var(--line)",
                    textAlign: "center",
                  }}
                >
                  <div className="field-label" style={{ marginBottom: 8 }}>
                    Unlock code
                  </div>
                  <div
                    className="big-number"
                    style={{
                      fontFamily: "ui-monospace, monospace",
                      letterSpacing: "0.12em",
                    }}
                  >
                    {formatUnlockCode(view.code)}
                  </div>
                  <p className="card-sub" style={{ margin: "10px 0 0" }}>
                    Type this into the device to unlock it.
                  </p>
                </div>
              ) : view.kind === "error" ? (
                <Banner tone="warn">
                  Your guardian key couldn't be read on this phone, so no unlock
                  code can be made. Restore it from your backup and try again.
                </Banner>
              ) : (
                <p className="card-sub" style={{ margin: 0, textAlign: "center" }}>
                  Enter all {UNLOCK_CHALLENGE_LEN} characters from the device's
                  screen to see the unlock code.
                </p>
              )}
            </div>

            <div className="stack" style={{ marginTop: 20 }}>
              <Button variant="secondary" block onClick={onClose}>
                Done
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
