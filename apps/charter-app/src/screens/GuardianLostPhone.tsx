import { useState } from "react";
import { Button, Card } from "../components/ui";
import { useCharter } from "../store/store";
import { SendSummary, type ReportLike } from "./GuardianSendSummary";
import { familyWards, rootsView } from "./guardiansView";

/**
 * A lost or replaced phone: withdraw its working key from every device, for
 * good. Lists only this guardian's other phones (keys the devices hold under
 * your identity); the other parent's phones are theirs to withdraw.
 */
export default function GuardianLostPhone({ onBack }: { onBack: () => void }) {
  const { state, guardianKeys } = useCharter();
  const wards = familyWards(state.children);
  const names = Object.fromEntries(wards.map((w) => [w.device.devicePubkey, w.device.label]));
  const me = guardianKeys.rootPubkey();
  const mine = rootsView(wards, me).find((r) => r.you);
  const others = (mine?.keys ?? []).filter((k) => k !== guardianKeys.workingPubkey);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ReportLike | null>(null);

  async function withdraw(w: string) {
    setBusy(true);
    setError(null);
    try {
      setReport(await guardianKeys.withdrawInstall(w));
      setConfirming(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (report) {
    return (
      <Card>
        <h2 className="card-title">Phone withdrawn</h2>
        <SendSummary report={report} names={names} />
        <p className="card-sub">Each device stops trusting that phone as soon as it has this.</p>
        <Button block onClick={onBack}>
          Done
        </Button>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Lost a phone?</h2>
      {!guardianKeys.state.root ? (
        <p className="card-sub">
          Your guardian key is kept on this phone only, so there are no other phones to withdraw. If you lose this
          phone, restore your key backup on a new one.
        </p>
      ) : others.length === 0 ? (
        <p className="card-sub">Your devices don't list any phone of yours other than this one.</p>
      ) : (
        <div className="stack" style={{ marginTop: 8 }}>
          <p className="card-sub" style={{ margin: 0 }}>
            Your other phones with Kintrinsic, as your child's devices know them. Withdraw one you've lost or given
            away.
          </p>
          {others.map((w) => (
            <div key={w} className="stack" style={{ gap: 6 }}>
              <div className="row-between">
                <span>Phone {w.slice(0, 8)}</span>
                {confirming !== w && (
                  <Button variant="secondary" disabled={busy} onClick={() => setConfirming(w)}>
                    Withdraw
                  </Button>
                )}
              </div>
              {confirming === w && (
                <div className="banner banner-warn">
                  <p style={{ margin: "0 0 8px" }}>
                    That phone will stop receiving anything at once, and can't sign for you again. This can't be
                    undone: a found phone has to be approved afresh.
                  </p>
                  <div className="stack">
                    <Button variant="danger" block disabled={busy} onClick={() => void withdraw(w)}>
                      {busy ? "Withdrawing…" : "Withdraw that phone for good"}
                    </Button>
                    <Button variant="ghost" block disabled={busy} onClick={() => setConfirming(null)}>
                      Keep it
                    </Button>
                  </div>
                </div>
              )}
            </div>
          ))}
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
