import { useState } from "react";
import { Button, Card } from "../components/ui";
import { useCharter } from "../store/store";
import { parseGuardianKey } from "../keys/guardianKeys";
import { PresenceWhere } from "./PresenceScanner";
import { familyWards, npubOf, stripNostrPrefix } from "./guardiansView";

/**
 * The invited parent's side: connect My Signet (or a Heartwood through it)
 * as this phone's guardian identity, have it approve this phone once, then
 * confirm in person at each of the child's devices.
 */
export default function GuardianJoin({ onBack, onConfirm }: { onBack: () => void; onConfirm: () => void }) {
  const { state, pairSignet, guardianKeys } = useCharter();
  const signer = state.signer;
  const [via, setVia] = useState<"signet" | "heartwood" | null>(null);
  const [invited, setInvited] = useState("");
  const [uri, setUri] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const viaName = via === "heartwood" || signer.kind === "heartwood" ? "Heartwood" : "My Signet";

  /** The npub the other parent invited, if given; undefined when blank, null when unreadable. */
  function expected(): string | undefined | null {
    const t = invited.trim();
    if (!t) return undefined;
    return parseGuardianKey(stripNostrPrefix(t));
  }

  async function run(fn: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const errorLine = error && (
    <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
      {error}
    </p>
  );

  // A phone that keeps its own guardian key already governs (or could govern)
  // devices with it. Swapping in My Signet under them would leave those
  // devices refusing everything, so joining starts from a phone without one.
  if (signer.kind === "local") {
    const hasDevices = familyWards(state.children).length > 0;
    return (
      <Card>
        <h2 className="card-title">Join a family</h2>
        <p className="card-sub">
          {hasDevices
            ? "This phone already looks after devices with a guardian key of its own, so it can't join another parent's family. Use Kintrinsic on another phone to join."
            : "This phone keeps a guardian key of its own. To join another parent's family with My Signet, turn off parent approval first (Family → Parent approval)."}
        </p>
        <Button variant="ghost" block onClick={onBack}>
          Back
        </Button>
      </Card>
    );
  }

  const root = guardianKeys.state.root;
  if (root) {
    const roster = guardianKeys.unconfirmedRosterDevices();
    return (
      <Card>
        <h2 className="card-title">Now confirm in person</h2>
        <p className="card-sub">
          This phone is approved for your guardian identity. You become a guardian of each device once you confirm in
          person there, within a week of the invitation.
        </p>
        <p className="card-sub">
          Your guardian ID: <span style={{ overflowWrap: "anywhere", userSelect: "all" }}>{npubOf(root.pubkey)}</span>.
          If it isn't the one you gave the other parent, the devices won't accept you: ask them to invite this one.
        </p>
        <ol className="card-sub" style={{ paddingLeft: 20, margin: "4px 0 8px" }}>
          <li>Go to the child's device.</li>
          <li>Ask it for a presence code (below).</li>
          <li>Scan it with this phone.</li>
        </ol>
        <PresenceWhere />
        <p className="card-sub">
          A ward phone asks for the guardian PIN first: the parent who invited you knows it. Repeat at every device.
        </p>
        {roster.length > 0 && (
          <div className="stack" style={{ gap: 4, marginTop: 8 }}>
            <div style={{ fontWeight: 600 }}>Devices still to confirm</div>
            {roster.map((d) => (
              <p key={d.machine} className="card-sub" style={{ margin: 0 }}>
                {d.name} ({d.platform === "android" ? "phone" : "computer"})
              </p>
            ))}
          </div>
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

  // My Signet connected (or paired) but this phone isn't approved yet.
  if ((signer.kind === "signet" || signer.kind === "heartwood") && signer.bunkerUri) {
    return (
      <Card>
        <h2 className="card-title">Join a family</h2>
        <p className="card-sub">
          {viaName} is connected. Approve this phone once, so it can confirm in person and sign for you.
        </p>
        <div className="stack" style={{ marginTop: 12 }}>
          <InvitedField value={invited} onChange={setInvited} disabled={busy} />
          {errorLine}
          <Button
            block
            disabled={busy}
            onClick={() =>
              run(async () => {
                const want = expected();
                if (want === null) throw new Error("That isn't a guardian ID. It starts with npub1.");
                await guardianKeys.approveInstall(want ? { expectRoot: want } : {});
              })
            }
          >
            {busy ? `Waiting for ${viaName}…` : "Approve this phone"}
          </Button>
          <Button variant="ghost" block disabled={busy} onClick={onBack}>
            Back
          </Button>
        </div>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Join a family</h2>
      <p className="card-sub">
        For a parent the other guardian has invited. You need the guardian identity they invited, in My Signet (or on
        a Heartwood reached through My Signet).
      </p>
      {via === null ? (
        <div className="stack" style={{ marginTop: 12 }}>
          <Button block onClick={() => setVia("signet")}>
            Connect My Signet
          </Button>
          <Button variant="secondary" block onClick={() => setVia("heartwood")}>
            Connect a Heartwood
          </Button>
          <Button variant="ghost" block onClick={onBack}>
            Back
          </Button>
        </div>
      ) : (
        <div className="stack" style={{ marginTop: 12 }}>
          <p className="card-sub" style={{ margin: 0 }}>
            {via === "heartwood"
              ? "In My Signet, copy the connection link for your Heartwood identity and paste it here."
              : "In My Signet, copy your connection link and paste it here."}{" "}
            It will be asked once to approve this phone. Then check the guardian ID shown is the one you gave the
            other parent.
          </p>
          <label className="field">
            <span className="field-label">Connection link</span>
            <input
              className="input"
              placeholder="bunker://…"
              value={uri}
              disabled={busy}
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
              onChange={(e) => setUri(e.target.value)}
            />
          </label>
          {errorLine}
          <Button
            block
            disabled={busy || !uri.trim().startsWith("bunker://")}
            onClick={() =>
              run(async () => {
                const r = await pairSignet(uri.trim(), { via });
                if (!r.approved) throw new Error(`Connected, but this phone isn't approved yet: ${r.error}`);
              })
            }
          >
            {busy ? `Waiting for ${via === "heartwood" ? "Heartwood" : "My Signet"}…` : "Connect"}
          </Button>
          <Button variant="ghost" block disabled={busy} onClick={() => setVia(null)}>
            Back
          </Button>
        </div>
      )}
    </Card>
  );
}

function InvitedField({ value, onChange, disabled }: { value: string; onChange: (v: string) => void; disabled: boolean }) {
  return (
    <label className="field">
      <span className="field-label">The guardian ID you were invited as (optional)</span>
      <input
        className="input"
        placeholder="npub1…"
        value={value}
        disabled={disabled}
        autoCapitalize="none"
        autoCorrect="off"
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
      />
      <span className="card-sub">If you give it, Kintrinsic refuses any other identity.</span>
    </label>
  );
}
