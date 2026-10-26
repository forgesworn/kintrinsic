import { useState } from "react";
import type { Child, Device } from "../domain/types";
import { Button } from "../components/ui";
import { DEFAULT_RELAYS } from "../signer/config";
import { validateRelays } from "../wire/relayConfig";
import { useCharter } from "../store/store";
import { localRouteGapLine } from "../carrier/localRoute";
import { useLocalRoute } from "../carrier/useLocalRoute";

export default function DeviceRelays({ child, device }: { child: Child; device: Device }) {
  const { createRelayCode, recordDeviceRelays } = useCharter();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(() => (device.relays ?? DEFAULT_RELAYS).join("\n"));
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const routeLine = localRouteGapLine(useLocalRoute().gap);

  async function generate() {
    setError("");
    setCode("");
    setBusy(true);
    setCopied(false);
    try {
      const relays = validateRelays(draft.split(/[\s,]+/).filter(Boolean));
      setCode(await createRelayCode(child.id, device.id, relays));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not create the relay code.");
    } finally { setBusy(false); }
  }

  return (
    <div style={{ flexBasis: "100%" }}>
      <Button variant="ghost" aria-expanded={open} disabled={busy} onClick={() => {
        if (!open && !code) setDraft((device.relays ?? DEFAULT_RELAYS).join("\n"));
        setOpen(!open);
      }}>Relays</Button>
      {open && <div className="stack" style={{ marginTop: 8, gap: 10 }}>
        <label className="field">
          <span className="field-label">Relays for {device.label}</span>
          <textarea className="input" rows={4} value={draft} disabled={busy} onChange={(e) => {
            setDraft(e.target.value); setCode(""); setError("");
          }} />
        </label>
        <p className="card-sub">Use at least two relays, one wss:// address per line. Choose relays run by different operators so an outage leaves another route open.</p>
        {routeLine && <p className="card-sub">{routeLine}</p>}
        <Button variant="secondary" disabled={busy} onClick={() => void generate()}>
          {busy ? "Signing…" : "Create relay code"}
        </Button>
        {error && <p role="alert" className="card-sub">{error}</p>}
        {code && <>
          <p className="card-sub">Apply this code on {device.label} within ten minutes. The old relay does not need to be working.</p>
          {device.platform === "android"
            ? <p className="card-sub">On the ward phone: Kintrinsic → About this device → Change relays. Paste the code and tap Apply.</p>
            : <p className="card-sub">On the computer, sign into your administrator account and open Kintrinsic → About this device. Paste the code and choose Apply relay change. You can also save it as <code>relay-code.json</code> and run <code>sudo charter-pair --relay-code &lt; relay-code.json</code>.</p>}
          <textarea aria-label={`Signed relay code for ${device.label}`} className="input" readOnly rows={4} value={code} />
          <Button variant="secondary" onClick={async () => {
            try { await navigator.clipboard.writeText(code); setCopied(true); }
            catch { setError("Select and copy the code above."); }
          }}>{copied ? "Copied" : "Copy code"}</Button>
          <p className="card-sub">After the device confirms the change, save the same relay list here.</p>
          <Button onClick={() => {
            const event = JSON.parse(code) as { created_at: number; content: string };
            const config = JSON.parse(event.content) as { relays: string[] };
            recordDeviceRelays(child.id, device.id, config.relays, event.created_at);
            setCode(""); setOpen(false);
          }}>The device applied this — save relays</Button>
        </>}
      </div>}
    </div>
  );
}
