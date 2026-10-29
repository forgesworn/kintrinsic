import { useState } from "react";
import { nip19 } from "nostr-tools";
import { Button, Card } from "../components/ui";
import { QRScanner } from "../components/QRScanner";
import { useCharter } from "../store/store";
import { parseGuardianKey } from "../keys/guardianKeys";
import { SendSummary, type ReportLike } from "./GuardianSendSummary";
import { GuardianCodeCard } from "./GuardianCode";
import { familyWards, myRootOf, stripNostrPrefix } from "./guardiansView";

/**
 * Invite the other parent: their guardian ID (npub), scanned from their My
 * Signet or pasted. Your identity signs an invitation to every device; it
 * takes effect at each one once they confirm in person there.
 */
export default function GuardianInvite({ onBack }: { onBack: () => void }) {
  const { state, guardianKeys } = useCharter();
  const wards = familyWards(state.children);
  const names = Object.fromEntries(wards.map((w) => [w.device.devicePubkey, w.device.label]));
  const hasAndroid = wards.some((w) => w.device.platform === "android");
  const [input, setInput] = useState("");
  const [scanning, setScanning] = useState(false);
  const [key, setKey] = useState<string | null>(null);
  const [label, setLabel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ReportLike | null>(null);

  function take(text: string): boolean {
    const k = parseGuardianKey(stripNostrPrefix(text));
    if (!k) {
      setError("That isn't a guardian ID. It starts with npub1.");
      return false;
    }
    if (k === myRootOf(guardianKeys) || k === guardianKeys.workingPubkey) {
      setError("That's your own guardian ID. Use the other parent's.");
      return false;
    }
    setError(null);
    setScanning(false);
    setKey(k);
    return true;
  }

  async function send() {
    if (!key) return;
    setBusy(true);
    setError(null);
    try {
      setReport(await guardianKeys.inviteCoGuardian(key, label.trim() ? { label: label.trim() } : {}));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (report) {
    return (
      <Card>
        <h2 className="card-title">Invitation sent</h2>
        <SendSummary report={report} names={names} />
        <p className="card-sub" style={{ marginTop: 12 }}>
          It lasts a week. Now the other parent:
        </p>
        <ol className="card-sub" style={{ paddingLeft: 20, margin: "4px 0 0" }}>
          <li>opens Kintrinsic on their phone and chooses Family → Guardians → Join a family;</li>
          <li>connects their My Signet (the identity you just invited);</li>
          <li>scans your guardian code (below) with their phone, so it trusts your family list and no one else's;</li>
          <li>
            visits each device, asks it for a presence code, and scans it with Kintrinsic (Family → Guardians →
            Confirm in person){wards.length > 0 ? `: ${wards.map((w) => w.device.label).join(", ")}` : ""}.
          </li>
        </ol>
        <p className="card-sub">
          Once they have confirmed at a device, open Kintrinsic on this phone: it shares the family (your children and
          their devices) with their phone, so they see it too.
        </p>
        <div style={{ fontWeight: 600, marginTop: 8 }}>Your guardian code</div>
        <GuardianCodeCard />
        <p className="card-sub">You can show it again later: Family → Guardians → Show my guardian code.</p>
        {hasAndroid && (
          <p className="card-sub">
            A ward phone asks for the guardian PIN before it shows the code. Tell them the PIN in person, never where
            your child can see or hear it.
          </p>
        )}
        <Button block onClick={onBack} style={{ marginTop: 12 }}>
          Done
        </Button>
      </Card>
    );
  }

  if (key) {
    const npub = nip19.npubEncode(key);
    return (
      <Card>
        <h2 className="card-title">Invite this parent?</h2>
        <p className="card-sub">Check this matches the guardian ID their My Signet shows:</p>
        <p style={{ overflowWrap: "anywhere", userSelect: "all", fontSize: 13 }}>{npub}</p>
        <p className="card-sub">
          Once they confirm in person, they have the same say as you on every device: they can set limits, answer
          requests and invite or remove a guardian.
        </p>
        <div className="stack" style={{ marginTop: 12 }}>
          <label className="field">
            <span className="field-label">What to call them (optional)</span>
            <input
              className="input"
              value={label}
              maxLength={40}
              disabled={busy}
              placeholder="e.g. Dad"
              onChange={(e) => setLabel(e.target.value)}
            />
          </label>
          {error && (
            <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
              {error}
            </p>
          )}
          <Button block disabled={busy} onClick={() => void send()}>
            {busy ? "Sending…" : "Send the invitation"}
          </Button>
          <Button variant="ghost" block disabled={busy} onClick={() => setKey(null)}>
            Back
          </Button>
        </div>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Invite a parent</h2>
      <p className="card-sub">
        The other parent needs My Signet on their phone. Ask them for their guardian ID: in My Signet it's the npub
        under their name, with a QR code.
      </p>
      <div className="stack" style={{ marginTop: 12 }}>
        {scanning ? (
          <>
            <QRScanner
              active={scanning}
              // A stray QR keeps the camera scanning, without an error each frame.
              onScan={(data) => (parseGuardianKey(stripNostrPrefix(data)) ? take(data) : false)}
            />
            <Button variant="ghost" block onClick={() => setScanning(false)}>
              Stop scanning
            </Button>
          </>
        ) : (
          <Button block onClick={() => setScanning(true)}>
            Scan their QR code
          </Button>
        )}
        <label className="field">
          <span className="field-label">Or paste their guardian ID</span>
          <input
            className="input"
            value={input}
            placeholder="npub1…"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            onChange={(e) => {
              setInput(e.target.value);
              setError(null);
            }}
          />
        </label>
        {error && (
          <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
            {error}
          </p>
        )}
        <Button variant="secondary" block disabled={!input.trim()} onClick={() => take(input)}>
          Next
        </Button>
        <Button variant="ghost" block onClick={onBack}>
          Cancel
        </Button>
      </div>
    </Card>
  );
}
