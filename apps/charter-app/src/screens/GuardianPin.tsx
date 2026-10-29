import { useState } from "react";
import { Button, Card } from "../components/ui";
import { useCharter } from "../store/store";
import { MIN_PIN_LENGTH } from "../keys/pin";
import { SendSummary, type ReportLike } from "./GuardianSendSummary";
import { pinProblem, type WardRef } from "./guardiansView";

/**
 * Set the guardian PIN on the family's Android wards. A ward phone asks for
 * it before it shows a presence code, so nothing that needs a guardian in
 * person can happen there without it. Only the hash reaches the ward; this
 * phone doesn't keep the PIN either.
 */
export default function GuardianPin({
  wards,
  onDone,
  onCancel,
}: {
  /** The Android wards it goes to (for the names in the summary). */
  wards: WardRef[];
  onDone: () => void;
  onCancel?: () => void;
}) {
  const { guardianKeys } = useCharter();
  const [pin, setPin] = useState("");
  const [again, setAgain] = useState("");
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ReportLike | null>(null);
  const problem = pinProblem(pin, again);
  const names = Object.fromEntries(wards.map((w) => [w.device.devicePubkey, w.device.label]));

  async function send() {
    setTouched(true);
    if (problem) return;
    setBusy(true);
    setError(null);
    try {
      const r = await guardianKeys.setGuardianPin(pin);
      setReport(r);
      setPin("");
      setAgain("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (report) {
    const reached = report.sent.length + report.held.length > 0;
    return (
      <Card>
        <h2 className="card-title">{reached ? "Guardian PIN sent" : "The PIN didn't reach any phone"}</h2>
        <SendSummary report={report} names={names} />
        <div className="stack" style={{ marginTop: 12 }}>
          {reached ? (
            <Button block onClick={onDone}>
              Continue
            </Button>
          ) : (
            <Button block onClick={() => setReport(null)}>
              Try again
            </Button>
          )}
        </div>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Set a guardian PIN</h2>
      <p className="card-sub">
        {wards.length === 1 ? wards[0].device.label : "Your child's phones"} will ask for this PIN before showing a
        code to confirm a guardian in person. Your child must not see it, so choose it somewhere they can't watch.
      </p>
      <p className="card-sub">
        At least {MIN_PIN_LENGTH} digits. Kintrinsic doesn't keep it: write it down somewhere safe, and tell the
        other parent in person if they will confirm at a phone.
      </p>
      <div className="stack" style={{ marginTop: 12 }}>
        <label className="field">
          <span className="field-label">Guardian PIN</span>
          <input
            className="input"
            type="password"
            inputMode="numeric"
            autoComplete="new-password"
            value={pin}
            disabled={busy}
            onChange={(e) => setPin(e.target.value.trim())}
          />
        </label>
        <label className="field">
          <span className="field-label">The same PIN again</span>
          <input
            className="input"
            type="password"
            inputMode="numeric"
            autoComplete="new-password"
            value={again}
            disabled={busy}
            onChange={(e) => setAgain(e.target.value.trim())}
          />
        </label>
        {touched && problem && (
          <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
            {problem}
          </p>
        )}
        {error && (
          <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
            {error}
          </p>
        )}
        <Button block disabled={busy} onClick={() => void send()}>
          {busy ? "Sending…" : "Set guardian PIN"}
        </Button>
        {onCancel && (
          <Button variant="ghost" block disabled={busy} onClick={onCancel}>
            Cancel
          </Button>
        )}
      </div>
    </Card>
  );
}
