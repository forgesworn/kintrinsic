import { useState } from "react";
import { Button, Card } from "../components/ui";
import { useCharter } from "../store/store";
import { SendSummary, type ReportLike } from "./GuardianSendSummary";
import { familyWards, moveGate, shortNpub } from "./guardiansView";

/**
 * Change my identity: move this guardian's root to another My Signet or
 * Heartwood identity. The old one signs the move, the new one approves this
 * phone, and each device switches once this phone confirms in person there.
 */
export default function GuardianMove({ onBack, onConfirm }: { onBack: () => void; onConfirm: () => void }) {
  const { state, guardianKeys } = useCharter();
  const wards = familyWards(state.children);
  const names = Object.fromEntries(wards.map((w) => [w.device.devicePubkey, w.device.label]));
  const gate = moveGate(guardianKeys.state, guardianKeys.familyReadiness().ready);
  const [via, setVia] = useState<"signet" | "heartwood">("signet");
  const [uri, setUri] = useState("");
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

  if (report || next) {
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
          <p className="card-sub" style={{ margin: 0 }}>
            Moves you to another guardian identity, for example a new My Signet or a Heartwood. Your current identity
            signs the move, the new one approves this phone, and then you confirm in person at every device. The other
            parent isn't affected.
          </p>
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
                Both identities will be asked to sign. After that, visit each device and confirm in person; a device
                you don't visit within a week keeps your current identity.
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
      {error && (
        <p role="alert" className="card-sub" style={{ color: "var(--danger, #B23B30)" }}>
          {error}
        </p>
      )}
      <Button variant="ghost" block onClick={onBack} style={{ marginTop: 12 }}>
        Back
      </Button>
    </Card>
  );
}
