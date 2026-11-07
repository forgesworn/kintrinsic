import { useState } from "react";
import { Button, Card } from "../components/ui";
import { AnchorScanFields } from "./AnchorScan";
import { useCharter } from "../store/store";
import { parseGuardianKey } from "../keys/guardianKeys";
import { PresenceWhere } from "./PresenceScanner";
import { GuardianCodeCard } from "./GuardianCode";
import PhoneJoinNote from "./PhoneJoinNote";
import { familyWards, JOIN_WITH_PHONE, JOIN_WITH_SIGNET, npubOf, shortNpub, stripNostrPrefix } from "./guardiansView";

/**
 * The invited parent's side. Either connect My Signet (or a Heartwood
 * through it) as this phone's guardian identity and have it approve this
 * phone once, or join with this phone only: its own key becomes the
 * identity and approves a separate day-to-day key (decision 1(b), D5), and
 * the other parent invites it from this phone's guardian code. Then scan
 * the inviting parent's guardian code (whose family list this phone will
 * trust), and confirm in person at each of the child's devices.
 */
export default function GuardianJoin({ onBack, onConfirm }: { onBack: () => void; onConfirm: () => void }) {
  const { state, pairSignet, enableLocalSigner, guardianKeys } = useCharter();
  const signer = state.signer;
  const [path, setPath] = useState<"signet" | null>(null);
  const [via, setVia] = useState<"signet" | "heartwood" | null>(null);
  const [invited, setInvited] = useState("");
  const [uri, setUri] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [ownSecondPhone, setOwnSecondPhone] = useState(false);
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

  const wards = familyWards(state.children);
  // A phone that keeps its own guardian key and governs devices it paired
  // itself can't join another family: its own children would travel into
  // the other parent's roster. A parent who joined with this phone only has
  // devices from the other parent's roster alone, and carries on below.
  if (signer.kind === "local" && wards.some((w) => !w.device.fromRoster)) {
    return (
      <Card>
        <h2 className="card-title">Join a family</h2>
        <p className="card-sub">
          This phone already looks after devices with a guardian key of its own, so it can't join another parent's
          family. Use Kintrinsic on another phone to join.
        </p>
        <Button variant="ghost" block onClick={onBack}>
          Back
        </Button>
      </Card>
    );
  }

  const root = guardianKeys.state.root;
  const phoneRoot = root?.via === "phone";
  const anchors = guardianKeys.trustedRoots().filter((r) => r !== root?.pubkey && r !== guardianKeys.state.nextRoot?.pubkey);
  if (root && anchors.length === 0 && !ownSecondPhone && wards.length === 0) {
    return (
      <Card>
        <h2 className="card-title">{phoneRoot ? "Swap guardian codes" : "Scan the other parent's guardian code"}</h2>
        {phoneRoot && (
          <>
            <p className="card-sub">
              First, the other parent invites you: on their phone, Settings → Guardians → Invite a parent → Scan their QR
              code, then they scan this one.
            </p>
            <GuardianCodeCard intro="This is your guardian ID for the family, kept on this phone. Show it to the other parent in person." />
          </>
        )}
        <p className="card-sub">
          {phoneRoot ? "Then scan theirs. " : ""}This phone shares the family only with the parent who invited you. On
          their phone, they open Settings → Guardians → Show my guardian code. Scan it here, in person.
        </p>
        <div className="stack" style={{ marginTop: 12 }}>
          <AnchorScanFields />
          {/* A phone-held identity's second phone is a restore from its key backup, not an approval. */}
          {!phoneRoot && (
            <Button variant="ghost" block onClick={() => setOwnSecondPhone(true)}>
              This is a second phone for my own identity
            </Button>
          )}
          <Button variant="ghost" block onClick={onBack}>
            Back
          </Button>
        </div>
      </Card>
    );
  }
  if (root) {
    const roster = guardianKeys.unconfirmedRosterDevices();
    return (
      <Card>
        <h2 className="card-title">Now confirm in person</h2>
        <p className="card-sub">
          {phoneRoot
            ? "This phone holds your guardian identity and its day-to-day key."
            : "This phone is approved for your guardian identity."}{" "}
          You become a guardian of each device once you confirm in person there, within a week of the invitation.
        </p>
        {phoneRoot ? (
          <GuardianCodeCard intro="Your guardian ID, kept on this phone. If the other parent hasn't invited it yet, show them this to scan (Guardians → Invite a parent)." />
        ) : (
          <p className="card-sub">
            Your guardian ID: <span style={{ overflowWrap: "anywhere", userSelect: "all" }}>{npubOf(root.pubkey)}</span>.
            If it isn't the one you gave the other parent, the devices won't accept you: ask them to invite this one.
          </p>
        )}
        {anchors.length > 0 && (
          <p className="card-sub">
            Family list from: {anchors.map(shortNpub).join(", ")} (the guardian code you scanned).
          </p>
        )}
        <ol className="card-sub" style={{ paddingLeft: 20, margin: "4px 0 8px" }}>
          <li>Go to the child's device.</li>
          <li>Ask it for a presence code (below).</li>
          <li>Scan it with this phone.</li>
        </ol>
        <PresenceWhere />
        <p className="card-sub">
          A ward phone asks for the guardian PIN first: the parent who invited you knows it. Repeat at every device.
          Once you have confirmed at one, the other parent's phone shares the family with this one when they next open
          Kintrinsic.
        </p>
        {!phoneRoot && (
          <p className="card-sub">
            Adding a second phone for your own guardian identity? Nothing waits to be confirmed then. Instead connect one
            device by its device code: in Settings, add the child; on the child's settings choose Set up a device, then
            type the code the device shows. Your other phone then shares the rest of the family.
          </p>
        )}
        <PhoneJoinNote />
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

  // The choice: My Signet, or this phone only.
  if (path === null) {
    return (
      <Card>
        <h2 className="card-title">Join a family</h2>
        <p className="card-sub">For a parent the other guardian is inviting. How do you want to join?</p>
        <div className="stack" style={{ marginTop: 12 }}>
          {errorLine}
          <Button block disabled={busy} onClick={() => setPath("signet")}>
            {JOIN_WITH_SIGNET}
          </Button>
          <Button
            variant="secondary"
            block
            disabled={busy}
            onClick={() =>
              run(async () => {
                if (signer.kind !== "local") await enableLocalSigner();
                await guardianKeys.joinWithThisPhone();
              })
            }
          >
            {busy ? "Making your guardian identity…" : JOIN_WITH_PHONE}
          </Button>
          <Button variant="ghost" block disabled={busy} onClick={onBack}>
            Back
          </Button>
        </div>
      </Card>
    );
  }

  // My Signet chosen, on a phone that signs with its own key: that key would
  // have to go first.
  if (signer.kind === "local") {
    return (
      <Card>
        <h2 className="card-title">Join a family</h2>
        <p className="card-sub">
          This phone keeps a guardian key of its own. To join another parent's family with My Signet, turn off parent
          approval first (Settings → Parent approval).
        </p>
        <Button variant="ghost" block onClick={() => setPath(null)}>
          Back
        </Button>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Join with My Signet</h2>
      <p className="card-sub">
        You need the guardian identity the other parent invited, in My Signet (or on a Heartwood reached through My
        Signet).
      </p>
      {via === null ? (
        <div className="stack" style={{ marginTop: 12 }}>
          <Button block onClick={() => setVia("signet")}>
            Connect My Signet
          </Button>
          <Button variant="secondary" block onClick={() => setVia("heartwood")}>
            Connect a Heartwood
          </Button>
          <Button variant="ghost" block onClick={() => setPath(null)}>
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
          <InvitedField value={invited} onChange={setInvited} disabled={busy} required />
          {errorLine}
          <Button
            block
            disabled={busy || !uri.trim().startsWith("bunker://") || !invited.trim()}
            onClick={() =>
              run(async () => {
                const want = expected();
                // Required here: the identity is checked before My Signet
                // is asked to sign anything (batch 3b review).
                if (!want) throw new Error("Enter the guardian ID you were invited as. It starts with npub1.");
                // Checked before anything is approved: a different identity is refused and not left connected.
                const r = await pairSignet(uri.trim(), { via, ...(want ? { expectRoot: want } : {}) });
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

function InvitedField({
  value,
  onChange,
  disabled,
  required = false,
}: {
  value: string;
  onChange: (v: string) => void;
  disabled: boolean;
  required?: boolean;
}) {
  return (
    <label className="field">
      <span className="field-label">The guardian ID you were invited as{required ? "" : " (optional)"}</span>
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
      <span className="card-sub">
        {required
          ? "Kintrinsic checks it before anything is signed, and refuses any other identity."
          : "If you give it, Kintrinsic refuses any other identity."}
      </span>
    </label>
  );
}
