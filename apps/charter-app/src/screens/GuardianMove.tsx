import { useState } from "react";
import { Button, Card } from "../components/ui";
import { useCharter } from "../store/store";
import { SendSummary, type ReportLike } from "./GuardianSendSummary";
import { familyWards, MINT_NOTICE, moveGate, moveNeedsFinishing, shortNpub, takeMoveOffer } from "./guardiansView";
import { phoneHeldRoot } from "../keys/state";

/**
 * Change my identity: move this guardian's root to another My Signet or
 * Heartwood identity, from My Signet or from this phone's own key. The old
 * one signs the move, the new one approves this phone's day-to-day key, and
 * each device switches once this phone confirms in person there. After a
 * move from this phone's own key, the old key may be deleted once every
 * device follows the new identity.
 */
export default function GuardianMove({ onBack, onConfirm }: { onBack: () => void; onConfirm: () => void }) {
  const { state, guardianKeys } = useCharter();
  const wards = familyWards(state.children);
  const names = Object.fromEntries(wards.map((w) => [w.device.devicePubkey, w.device.label]));
  const progress = guardianKeys.moveProgress();
  const gate = moveGate(guardianKeys.state, guardianKeys.familyReadiness().ready, progress);
  const [offer] = useState(takeMoveOffer);
  const [via, setVia] = useState<"signet" | "heartwood">(offer?.via ?? "signet");
  const [uri, setUri] = useState(offer?.uri ?? "");
  const fromPhone = phoneHeldRoot(guardianKeys.state);
  const [retiring, setRetiring] = useState(false);
  const [retired, setRetired] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ReportLike | null>(null);
  const next = guardianKeys.state.nextRoot;

  async function move() {
    setBusy(true);
    setError(null);
    try {
      setReport(await guardianKeys.moveRoot(uri.trim(), via));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function finish() {
    setBusy(true);
    setError(null);
    try {
      setReport(await guardianKeys.finishMove());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  function retire() {
    setError(null);
    try {
      guardianKeys.retirePhoneKey();
      setRetired(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  const errorLine = error && (
    <p role="alert" className="card-sub" style={{ color: "var(--danger, #B23B30)" }}>
      {error}
    </p>
  );

  // Lapsed (expired or cancelled) after some devices switched: those pin the
  // new identity and the rest the old, and nothing the old one signs reaches
  // the switched ones. Restarting the move to the same identity finishes it.
  if (!report && next && moveNeedsFinishing(progress)) {
    return (
      <Card>
        <h2 className="card-title">Finish your identity move</h2>
        <p className="card-sub">
          Your move to {shortNpub(next.pubkey)} lapsed before every device switched. {progress!.moved.join(", ")}{" "}
          {progress!.moved.length === 1 ? "already uses" : "already use"} the new identity; your other devices still use
          the old one. Until the move is finished you can't invite, remove or leave.
        </p>
        <p className="card-sub">
          Finishing sends the move again: both identities are asked to sign, then you confirm in person at the devices
          that haven't switched, within a week. The new identity's {next.via === "heartwood" ? "Heartwood" : "My Signet"}{" "}
          must be reachable.
        </p>
        {errorLine}
        <div className="stack" style={{ marginTop: 12 }}>
          <Button block disabled={busy} onClick={() => void finish()}>
            {busy ? "Waiting for approval…" : "Finish the move"}
          </Button>
          <Button variant="ghost" block disabled={busy} onClick={onBack}>
            Back
          </Button>
        </div>
      </Card>
    );
  }

  // A move from this phone's own key has landed on every device: the old key
  // is still here. Deleting it is offered, never done for the guardian.
  if (!report && (retired || guardianKeys.phoneKeyRetirable())) {
    return (
      <Card>
        <h2 className="card-title">{retired ? "Old key deleted" : "Your move is finished"}</h2>
        {retired ? (
          <p className="card-sub">
            This phone now holds only its day-to-day key, approved by your new identity. Make a new backup in Settings:
            older key backups no longer bring your family back.
          </p>
        ) : (
          <>
            <p className="card-sub">
              Every device now follows your new identity and takes this phone's day-to-day key under it. Your old
              guardian key is still on this phone, but nothing uses it any more.
            </p>
            {retiring ? (
              <div className="banner banner-warn">
                <p style={{ margin: "0 0 8px" }}>
                  This deletes your old guardian key from this phone for good. Backups made before the move hold that old
                  key and will no longer bring your family back; make a new one afterwards. A device you still have to
                  pair again under the old key can't be reached with it. This can't be undone.
                </p>
                <div className="stack">
                  <Button variant="danger" block onClick={retire}>
                    Delete the old key
                  </Button>
                  <Button variant="ghost" block onClick={() => setRetiring(false)}>
                    Keep it for now
                  </Button>
                </div>
              </div>
            ) : (
              <Button block onClick={() => setRetiring(true)}>
                Delete the old key from this phone
              </Button>
            )}
          </>
        )}
        {errorLine}
        <Button variant="ghost" block onClick={onBack} style={{ marginTop: 12 }}>
          Back
        </Button>
      </Card>
    );
  }

  if (report || (next && !progress?.lapsed)) {
    return (
      <Card>
        <h2 className="card-title">Move waiting for you in person</h2>
        {report && <SendSummary report={report} names={names} />}
        {next && (
          <p className="card-sub">
            Moving to {shortNpub(next.pubkey)}. Each device switches once you confirm in person there, within a week.
            Until then your current identity stays in charge.
          </p>
        )}
        <div className="stack" style={{ marginTop: 12 }}>
          <Button block onClick={onConfirm}>
            Confirm in person
          </Button>
          <Button variant="ghost" block onClick={onBack}>
            Back
          </Button>
        </div>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Change my identity</h2>
      {!gate.ok ? (
        <p className="card-sub">{gate.reason}</p>
      ) : (
        <div className="stack" style={{ marginTop: 8 }}>
          {next && progress?.lapsed && (
            <p className="card-sub" style={{ margin: 0 }}>
              Your move to {shortNpub(next.pubkey)} lapsed before any device switched, so nothing changed. You can
              start again.
            </p>
          )}
          {fromPhone ? (
            <>
              <p className="card-sub" style={{ margin: 0 }}>
                Moves your guardian identity from this phone's own key to My Signet or a Heartwood, without pairing
                anything again. This phone's key signs the move and the new identity approves this phone. Then you
                confirm in person at each of your child's devices within a week; each one switches as you confirm
                there. Until then, this phone's key stays in charge. The other parent isn't affected.
              </p>
              {!guardianKeys.state.root && (
                <p className="card-sub" style={{ margin: 0 }}>
                  {MINT_NOTICE}
                </p>
              )}
              <p className="card-sub" style={{ margin: 0 }}>
                When every device has switched, you can delete the old key from this phone.
              </p>
            </>
          ) : (
            <p className="card-sub" style={{ margin: 0 }}>
              Moves you to another guardian identity, for example a new My Signet or a Heartwood. Your current My Signet
              approves the move and the new one approves this phone. Then you confirm in person at each of your
              child's devices within a week; each one switches as you confirm there. The other parent isn't affected.
            </p>
          )}
          <div className="seg" role="group" aria-label="New identity is in">
            <button type="button" className="seg-btn" aria-pressed={via === "signet"} onClick={() => setVia("signet")}>
              My Signet
            </button>
            <button
              type="button"
              className="seg-btn"
              aria-pressed={via === "heartwood"}
              onClick={() => setVia("heartwood")}
            >
              Heartwood
            </button>
          </div>
          <label className="field">
            <span className="field-label">The new identity's connection link</span>
            <input
              className="input"
              placeholder="bunker://…"
              value={uri}
              disabled={busy}
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              onChange={(e) => {
                setUri(e.target.value);
                setConfirming(false);
              }}
            />
          </label>
          {confirming ? (
            <div className="banner banner-warn">
              <p style={{ margin: "0 0 8px" }}>
                {fromPhone ? "Your new identity will be asked to approve this phone." : "Both identities will be asked to sign."}{" "}
                After that, visit each device and confirm in person; a device you don't visit within a week keeps your
                current identity.
              </p>
              <Button block disabled={busy} onClick={() => void move()}>
                {busy ? "Waiting for approval…" : "Start the move"}
              </Button>
            </div>
          ) : (
            <Button block disabled={busy || !uri.trim().startsWith("bunker://")} onClick={() => setConfirming(true)}>
              Next
            </Button>
          )}
        </div>
      )}
      {errorLine}
      <Button variant="ghost" block onClick={onBack} style={{ marginTop: 12 }}>
        Back
      </Button>
    </Card>
  );
}
