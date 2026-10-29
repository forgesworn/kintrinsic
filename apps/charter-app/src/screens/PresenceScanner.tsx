import { useState } from "react";
import { Button, Card } from "../components/ui";
import { QRScanner } from "../components/QRScanner";
import { useCharter } from "../store/store";
import { parsePresenceCode, parsePresenceToken } from "../keys/presenceCode";
import {
  familyWards,
  myRootOf,
  pendingFor,
  pendingTitle,
  presenceCodeFrom,
} from "./guardiansView";

/** Where each kind of ward shows its presence code. */
export function PresenceWhere() {
  return (
    <div className="stack" style={{ gap: 6 }}>
      <p className="card-sub" style={{ margin: 0 }}>
        <strong>On a computer:</strong> open Kintrinsic → Connect a phone → Confirm in person → Show the code to
        confirm. It asks for the computer's administrator password, then shows a code.
      </p>
      <p className="card-sub" style={{ margin: 0 }}>
        <strong>On a ward phone:</strong> open Kintrinsic → About this device → Guardians → Guardian presence. Pick
        the change, then Show a code for this. It asks for the guardian PIN, then shows a code.
      </p>
      <p className="card-sub" style={{ margin: 0 }}>Each code works once, for ten minutes.</p>
    </div>
  );
}

type Result = { ok: true; device: string; held: boolean; kind: string } | null;

/**
 * Confirm in person: scan the code a ward shows (or type its short form)
 * and send this phone's confirmation to that device. Only a code minted on
 * the device by an administrator or with the guardian PIN works, so a child
 * can't pass one on.
 */
export default function PresenceScanner({ onBack }: { onBack?: () => void }) {
  const { state, guardianKeys } = useCharter();
  const wards = familyWards(state.children);
  const me = myRootOf(guardianKeys);
  const [scanning, setScanning] = useState(false);
  const [typed, setTyped] = useState("");
  const [token, setToken] = useState<string | null>(null);
  const [machine, setMachine] = useState("");
  const [d, setD] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Result>(null);

  const deviceName = (m: string) => wards.find((w) => w.device.devicePubkey === m)?.device.label ?? "the device";

  async function confirm(code: string) {
    const parsed = parsePresenceCode(code);
    if (!parsed) {
      setError("That isn't a Kintrinsic presence code.");
      return;
    }
    setBusy(true);
    setError(null);
    setScanning(false);
    try {
      const r = await guardianKeys.confirmPresence(code);
      setResult({ ok: true, device: deviceName(parsed.machine), held: r.held.length > 0, kind: parsed.kind });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  function submitTyped() {
    setError(null);
    const text = typed.trim();
    if (text.startsWith("charter://")) {
      void confirm(text);
      return;
    }
    const t = parsePresenceToken(text);
    if (!t) {
      setError("That isn't a Kintrinsic presence code. The short form is 32 letters and numbers.");
      return;
    }
    setToken(t);
  }

  if (result) {
    return (
      <Card>
        <h2 className="card-title">Confirmation sent</h2>
        <p className="card-sub">
          {result.held
            ? `It's waiting for the home network, then goes to ${result.device}.`
            : `Sent to ${result.device}.`}{" "}
          {result.kind === "removal"
            ? "The removal still waits out its 72 hours' notice, and the other parent can object until then."
            : "It takes effect there as soon as the device has it."}{" "}
          The device's own screen shows it once it's done.
        </p>
        <div className="stack" style={{ marginTop: 12 }}>
          <Button
            block
            onClick={() => {
              setResult(null);
              setTyped("");
              setToken(null);
            }}
          >
            Confirm at another device
          </Button>
          {onBack && (
            <Button variant="ghost" block onClick={onBack}>
              Done
            </Button>
          )}
        </div>
      </Card>
    );
  }

  // The short form: which device and which pending item it is for. Only an
  // item this phone sent can be picked, since only then does it know the
  // statement the confirmation names; an invitation must be scanned.
  if (token) {
    const items = wards.flatMap((w) =>
      pendingFor(w.device.guardians, guardianKeys.state, me)
        // Only what this phone itself confirms: a removal it started, or its
        // own identity move. An invitation it sent is confirmed by the other
        // parent's phone, never this one.
        .filter((p) => p.statement && p.role === "mine" && p.kind !== "coguardian")
        .map((p) => ({ machine: w.device.devicePubkey, device: w.device.label, ...p })),
    );
    const devicesWithItems = [...new Map(items.map((i) => [i.machine, i.device])).entries()];
    const forDevice = items.filter((i) => i.machine === machine);
    const chosen = forDevice.find((i) => i.d === d);
    return (
      <Card>
        <h2 className="card-title">Which device and what for?</h2>
        {items.length === 0 ? (
          <p className="card-sub">
            The short code only works for something this phone asked for. To accept an invitation, scan the code on
            the device instead.
          </p>
        ) : (
          <div className="stack" style={{ marginTop: 8 }}>
            <label className="field">
              <span className="field-label">Device showing the code</span>
              <select
                className="select"
                value={machine}
                onChange={(e) => {
                  setMachine(e.target.value);
                  setD("");
                }}
              >
                <option value="">Choose a device</option>
                {devicesWithItems.map(([m, name]) => (
                  <option key={m} value={m}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            {machine && (
              <label className="field">
                <span className="field-label">What it's confirming</span>
                <select className="select" value={d} onChange={(e) => setD(e.target.value)}>
                  <option value="">Choose one</option>
                  {forDevice.map((i) => (
                    <option key={i.d} value={i.d}>
                      {pendingTitle(i, me)}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <Button
              block
              disabled={busy || !chosen}
              onClick={() => chosen && void confirm(presenceCodeFrom(chosen.machine, token, chosen.d, chosen.statement!))}
            >
              {busy ? "Sending…" : "Confirm in person"}
            </Button>
          </div>
        )}
        {error && (
          <p role="alert" className="card-sub" style={{ color: "var(--danger, #B23B30)" }}>
            {error}
          </p>
        )}
        <Button variant="ghost" block onClick={() => setToken(null)}>
          Back
        </Button>
      </Card>
    );
  }

  return (
    <Card>
      <h2 className="card-title">Confirm in person</h2>
      <p className="card-sub">
        Some changes only take effect once a guardian is at the child's device. Ask it for a code, then scan it here.
      </p>
      <PresenceWhere />
      <div className="stack" style={{ marginTop: 12 }}>
        {scanning ? (
          <>
            <QRScanner
              active={scanning}
              onScan={(data) => {
                // A stray QR keeps the camera scanning.
                if (!parsePresenceCode(data)) return false;
                void confirm(data);
                return true;
              }}
            />
            <Button variant="ghost" block onClick={() => setScanning(false)}>
              Stop scanning
            </Button>
          </>
        ) : (
          <Button block disabled={busy} onClick={() => setScanning(true)}>
            Scan the code
          </Button>
        )}
        <label className="field">
          <span className="field-label">Or type the code shown under it</span>
          <input
            className="input"
            value={typed}
            disabled={busy}
            placeholder="1a2b3c4d 5e6f…"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            onChange={(e) => {
              setTyped(e.target.value);
              setError(null);
            }}
          />
        </label>
        <Button variant="secondary" block disabled={busy || !typed.trim()} onClick={submitTyped}>
          {busy ? "Sending…" : "Use this code"}
        </Button>
        {error && (
          <p role="alert" className="card-sub" style={{ margin: 0, color: "var(--danger, #B23B30)" }}>
            {error}
          </p>
        )}
        {onBack && (
          <Button variant="ghost" block disabled={busy} onClick={onBack}>
            Back
          </Button>
        )}
      </div>
    </Card>
  );
}
