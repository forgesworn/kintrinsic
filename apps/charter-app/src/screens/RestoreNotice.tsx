import { useState } from "react";
import { Banner, Button } from "../components/ui";
import { useCharter } from "../store/store";

/**
 * After the household was restored from a backup (decision D5): this phone
 * has its own new day-to-day key, which an identity in My Signet approves,
 * and the lost phone's key is still trusted by the devices until it is
 * withdrawn. Shown until the guardian is done.
 */
export default function RestoreNotice() {
  const { state, guardianKeys } = useCharter();
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const follow = guardianKeys.restoreFollowUp();
  if (!follow) return null;
  const signet = state.signer.kind === "signet" || state.signer.kind === "heartwood";
  const approve = signet && !guardianKeys.state.root;
  const where = state.signer.kind === "heartwood" ? "My Signet (for your Heartwood)" : "My Signet";

  async function run(what: string, fn: () => Promise<unknown>, done: string) {
    setBusy(what);
    setNote(null);
    try {
      await fn();
      setNote(done);
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div style={{ marginBottom: 12 }}>
      <Banner tone="info">
        Your family was restored from a backup.{" "}
        {approve
          ? `This phone has a new day-to-day key of its own. Approve it in ${where} so it can sign for you.`
          : follow.lost.length > 0
            ? "Your devices still trust the lost phone's key."
            : "Nothing else to do."}
        {follow.lost.length > 0 && !approve && " Withdraw it so it can't sign for you again; no one needs to be at the devices."}
        <div className="stack" style={{ marginTop: 8, gap: 6 }}>
          {approve && (
            <Button
              disabled={busy !== null}
              onClick={() => void run("approve", () => guardianKeys.approveInstall(), "This phone is approved.")}
            >
              {busy === "approve" ? "Waiting for approval…" : "Approve this phone"}
            </Button>
          )}
          {!approve &&
            follow.lost.map((k) => (
              <Button
                key={k.pubkey}
                variant="secondary"
                disabled={busy !== null}
                onClick={() =>
                  void run(k.pubkey, () => guardianKeys.withdrawInstall(k.pubkey), "Withdrawn. Each device stops trusting it when it next checks in.")
                }
              >
                {busy === k.pubkey ? "Withdrawing…" : `Withdraw ${k.label ? `"${k.label}"` : `phone ${k.pubkey.slice(0, 8)}`}`}
              </Button>
            ))}
          <Button variant="ghost" disabled={busy !== null} onClick={() => guardianKeys.forgetRestore()}>
            Done
          </Button>
          {note && <span role="status">{note}</span>}
        </div>
      </Banner>
    </div>
  );
}
