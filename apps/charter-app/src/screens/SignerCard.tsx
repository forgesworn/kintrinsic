import { useState } from "react";
import { Button, Card, Pill } from "../components/ui";
import { useCharter } from "../store/store";
import { guardianPubkeyHex } from "../signer/guardianKey";
import { guardianBunkerUri, guardianNpub } from "../signer/guardianPairing";
import { DEFAULT_RELAYS } from "../signer/config";
import ApprovalSetupChoices from "./ApprovalSetupChoices";
import { KEY_BACKUP_ANCHOR } from "./KeyBackupNudge";
import KeyBackupSection from "./KeyBackupSection";

// The Parent approval card on Settings: whether this phone signs for the
// guardian (the built-in key, or My Signet / Heartwood), the pairing link, the
// auto-sign switch, and the key backup section for a key kept on this phone.

export default function SignerCard() {
  const { state, enableLocalSigner, pairSignet, disconnectSigner, setAutoSign, guardianKeys } = useCharter();
  const signer = state.signer;
  const [busy, setBusy] = useState(false);
  // Which root is being connected: My Signet, or a Heartwood behind My Signet
  // (the same bunker link; only the words differ). Null = not connecting.
  const [pairingVia, setPairingVia] = useState<"signet" | "heartwood" | null>(null);
  const pairing = pairingVia !== null;
  const [uri, setUri] = useState("");
  const [pairError, setPairError] = useState<string | null>(null);
  const [approvalNote, setApprovalNote] = useState<string | null>(null);
  const viaName = pairingVia === "heartwood" ? "your Heartwood" : "My Signet";

  // Anything a step throws is shown: turning on parent approval is refused
  // while My Signet holds the guardian identity, and that must be read.
  const [runError, setRunError] = useState<string | null>(null);
  async function run(fn: () => Promise<void>) {
    setBusy(true);
    setRunError(null);
    try {
      await fn();
    } catch (e) {
      setRunError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  const runErrorLine = runError && (
    <p role="alert" className="card-sub" style={{ margin: "8px 0 0", color: "var(--danger, #B23B30)" }}>
      {runError}
    </p>
  );

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        // clipboard unavailable — nothing more we can do
      }
      document.body.removeChild(ta);
    }
  }

  async function doPair() {
    setPairError(null);
    try {
      const result = await pairSignet(uri.trim(), { via: pairingVia ?? "signet" });
      setApprovalNote(
        result.approved
          ? null
          : `Connected. This phone isn't approved to sign for you yet, so ${viaName} will be asked each time: ${result.error}`,
      );
      setPairingVia(null);
      setUri("");
    } catch (e) {
      setPairError(e instanceof Error ? e.message : `Couldn't connect to ${viaName}.`);
    }
  }

  if (!signer.connected) {
    return (
      <Card>
        <h2 className="card-title">Turn on parent approval</h2>
        <p className="card-sub" style={{ marginBottom: 14 }}>
          Add a quick confirm step so only you can change limits or approve
          requests — a second lock on a parent's decisions. Choose the app
          you'll tap to confirm it's really you.
        </p>
        {pairing ? (
          <div className="stack">
            <p className="card-sub" style={{ margin: 0 }}>
              {pairingVia === "heartwood"
                ? "Your Heartwood is reached through My Signet. In My Signet, copy the connection link for your Heartwood identity and paste it here."
                : "In My Signet, copy your connection link and paste it here."}
            </p>
            <p className="card-sub" style={{ margin: 0 }}>
              {pairingVia === "heartwood" ? "Your Heartwood" : "My Signet"} becomes your guardian identity. It will be
              asked once to approve this phone, which then signs your changes itself.
            </p>
            <input
              className="input"
              placeholder="bunker://…"
              value={uri}
              disabled={busy}
              onChange={(e) => setUri(e.target.value)}
              autoCapitalize="none"
              autoCorrect="off"
              spellCheck={false}
            />
            {pairError && (
              <p className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
                {pairError}
              </p>
            )}
            <Button block disabled={busy || !uri.trim().startsWith("bunker://")} onClick={() => run(doPair)}>
              Connect
            </Button>
            <Button
              variant="ghost"
              block
              disabled={busy}
              onClick={() => {
                setPairingVia(null);
                setPairError(null);
              }}
            >
              Cancel
            </Button>
          </div>
        ) : (
          <ApprovalSetupChoices
            busy={busy}
            onLocal={() => run(enableLocalSigner)}
            onSignet={() => setPairingVia("signet")}
            onHeartwood={() => setPairingVia("heartwood")}
          />
        )}
        {runErrorLine}
        <p className="card-sub" style={{ marginTop: 12 }}>
          Simplest: “Set up on this phone” keeps the guardian key right here — no
          extra app. You can also set limits first; they’ll save now and apply
          once approval is on.
        </p>
      </Card>
    );
  }

  return (
    <Card>
      <div className="row-between">
        <h2 className="card-title" style={{ margin: 0 }}>
          Parent approval
        </h2>
        <Pill tone="ok">On</Pill>
      </div>
      {signer.kind === "local" && (() => {
        const pk = guardianPubkeyHex();
        const bunker = guardianBunkerUri(pk, DEFAULT_RELAYS);
        return (
          <div className="stack" style={{ marginTop: 12 }}>
            {/* This used to end "it links that laptop to this phone", which a
                second parent reasonably read as the way to add THEMSELVES —
                when the laptop being paired is a ward's device. A second parent
                joins with their own identity (Guardians), so naming whose
                computer this is matters more than the mechanics. */}
            <p className="card-sub" style={{ margin: 0 }}>
              To connect a computer your child uses: on that computer open
              Kintrinsic, go to “Connect a phone” and press “Show the pairing
              code”. Then, on this phone, add your child, choose “Set up a
              computer” and scan the code. That puts the computer under the
              rules you set here.
            </p>
            <p className="card-sub" style={{ margin: 0 }}>
              This isn’t how another parent joins. They join with their own
              identity: Settings → Guardians → Invite a parent.
            </p>
            <p className="card-sub" style={{ margin: 0 }}>
              No internet on either device? Copy this link instead and paste it
              into “No internet? Enter a code by hand” on the computer.
            </p>
            <code style={{ wordBreak: "break-all", fontSize: 12, opacity: 0.85 }}>{bunker}</code>
            <Button block disabled={busy} onClick={() => run(() => copyText(bunker))}>
              Copy pairing link
            </Button>
            <p className="card-sub" style={{ margin: 0, opacity: 0.7 }}>
              Your guardian ID: <span style={{ overflowWrap: "anywhere", userSelect: "all" }}>{guardianNpub(pk)}</span>
            </p>
            <Button variant="secondary" block disabled={busy} onClick={() => run(() => copyText(guardianNpub(pk)))}>
              Copy guardian npub
            </Button>
          </div>
        );
      })()}
      {(signer.kind === "signet" || signer.kind === "heartwood") && (
        <div className="stack" style={{ marginTop: 12 }}>
          {guardianKeys.state.root ? (
            <p className="card-sub" style={{ margin: 0 }}>
              This phone is approved to sign for your guardian identity
              ({guardianNpub(guardianKeys.state.root.pubkey).slice(0, 16)}…). Devices that have
              been updated take changes straight from this phone.
            </p>
          ) : (
            <>
              <p className="card-sub" style={{ margin: 0 }}>
                Approve this phone once in {signer.kind === "heartwood" ? "My Signet (for your Heartwood)" : "My Signet"},
                and it signs your changes itself, without asking each time.
              </p>
              <Button
                block
                disabled={busy}
                onClick={() =>
                  run(async () => {
                    try {
                      await guardianKeys.approveInstall();
                      setApprovalNote(null);
                    } catch (e) {
                      setApprovalNote(e instanceof Error ? e.message : String(e));
                    }
                  })
                }
              >
                Approve this phone
              </Button>
            </>
          )}
          {approvalNote && (
            <p className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
              {approvalNote}
            </p>
          )}
          <KeyBackupSection busy={busy} />
        </div>
      )}
      {runErrorLine}
      {signer.kind === "local" && (
        <div id={KEY_BACKUP_ANCHOR} style={{ scrollMarginTop: 80 }}>
          <KeyBackupSection busy={busy} />
        </div>
      )}

      <p className="card-sub" style={{ marginTop: 4 }}>
        Limit changes and approvals are confirmed by you
        {signer.label ? ` with ${signer.label}` : ""}.
      </p>

      <div
        className="row-between"
        style={{ marginTop: 16, gap: 12, alignItems: "flex-start" }}
      >
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600 }}>Approve without the extra tap</div>
          <p className="card-sub" style={{ marginTop: 2 }}>
            When on, your decisions go through right away. This only applies to
            choices you make here — it never approves your child's requests for
            you.
          </p>
        </div>
        <Switch
          checked={signer.autoSign}
          disabled={busy}
          label="Approve without the extra tap"
          onChange={(on) => run(() => setAutoSign(on))}
        />
      </div>

      <p className="card-sub" style={{ marginTop: 4 }}>
        You can turn this off any time.
      </p>

      <div style={{ marginTop: 16 }}>
        <Button
          variant="ghost"
          disabled={busy}
          onClick={() => run(() => disconnectSigner())}
        >
          Turn off parent approval
        </Button>
      </div>
    </Card>
  );
}

// ---------------------------------------------------------------------------
// Switch (accessible toggle)
// ---------------------------------------------------------------------------

function Switch({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        flex: "0 0 auto",
        appearance: "none",
        border: 0,
        cursor: disabled ? "default" : "pointer",
        width: 52,
        height: 32,
        borderRadius: 999,
        padding: 3,
        background: checked ? "var(--ok)" : "var(--line)",
        transition: "background-color 0.15s ease",
        opacity: disabled ? 0.6 : 1,
        display: "inline-flex",
        alignItems: "center",
      }}
    >
      <span
        aria-hidden="true"
        style={{
          display: "block",
          width: 26,
          height: 26,
          borderRadius: "50%",
          background: "#fff",
          boxShadow: "var(--shadow-1)",
          transform: checked ? "translateX(20px)" : "translateX(0)",
          transition: "transform 0.15s ease",
        }}
      />
    </button>
  );
}
